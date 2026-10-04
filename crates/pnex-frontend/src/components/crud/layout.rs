//! Layout canonique des pages « liste d'une ressource » — la coquille du
//! motif CRUD : header (titre, sous-titre, bouton retour, action « ajouter »
//! gated `can_write`), slot filtres, corps. Aucune décision visuelle ici :
//! les classes sont les littéraux historiques des pages (devices/flows),
//! repris à l'identique — y compris pour les const de boutons partagées
//! avec `form.rs`.
//!
//! Ce qui reste délibérément **à la page** : la garde org
//! (`org::current().is_none()` → message « orgs-empty »), le calcul
//! `can_write` (helper privé par page — convention projet) et l'i18n
//! (`t!` côté appelant, le socle ne connaît que des `String`).

use dioxus::prelude::*;

use crate::components::icons;

/// Bouton primaire — action « ajouter » du header, submit des formulaires.
pub const PRIMARY_BTN: &str = "px-4 py-2 bg-blue-600 text-white rounded-lg hover:bg-blue-700 transition-colors text-sm font-medium";

/// Bouton fantôme — annulation dans les pieds de formulaire.
pub const GHOST_BTN: &str = "px-4 py-2 text-sm text-gray-600 hover:text-gray-900 transition-colors";

/// Danger action — row-level "delete" of CRUD list tables (canonical
/// style: bordered red + trash icon, icon rendered by the caller since
/// the label stays i18n-resolved by the page).
pub const DANGER_BTN: &str = "inline-flex items-center px-3 py-1 text-sm text-red-700 bg-red-50 border border-red-200 rounded-lg hover:bg-red-100 transition-colors";

#[component]
pub fn ListLayout(
    /// Titre de la page (i18n résolu par l'appelant).
    title: String,
    /// Sous-titre optionnel (i18n résolu par l'appelant).
    #[props(default)]
    subtitle: Option<String>,
    /// Retour vers la liste parente — bouton borduré à l'icône agrandie
    /// (`h-5 w-5`, lisible face au titre `text-3xl` ; le `h-4 w-4`
    /// historique était trop discret).
    #[props(default)]
    on_back: Option<Callback<()>>,
    /// Libellé du bouton d'ajout (i18n résolu par l'appelant).
    #[props(default)]
    add_label: Option<String>,
    /// Clic « ajouter » — bouton primaire rendu ssi `can_write` et
    /// `on_add` + `add_label` fournis.
    #[props(default)]
    on_add: Option<Callback<()>>,
    /// Slot d'actions supplémentaires de l'en-tête (pages multi-boutons :
    /// media = capture caméra + take360 avant l'upload). Rendu à droite,
    /// avant le bouton « ajouter ».
    #[props(default)]
    actions: Option<Element>,
    /// Render the header row at all — full-page detail subviews that draw
    /// their own header (functions editor) pass `false` to drop the CRUD
    /// title / add-button row entirely.
    #[props(default = true)]
    header: bool,
    /// Droit d'écriture owner/admin — calculé par la page (le serveur
    /// force, l'UI masque).
    can_write: bool,
    /// Slot filtres rendu sous le header avec la classe canonique
    /// (`FilterBar` pour le même rendu dans le corps de page).
    #[props(default)]
    filters: Option<Element>,
    children: Element,
) -> Element {
    rsx! {
        // Phone: tighter gutters, bottom room so the floating assistant
        // button never covers the last row; desktop values from md up.
        div { class: "p-4 pb-24 md:p-6",
            if header {
                div { class: "mb-4 md:mb-8 flex items-center justify-between flex-wrap gap-3",
                    div { class: "flex items-center gap-3",
                        if let Some(back) = on_back {
                            button {
                                class: "inline-flex items-center gap-2 px-4 py-2.5 text-sm text-gray-600 border border-gray-300 rounded-lg hover:bg-gray-50 transition-colors",
                                r#type: "button",
                                onclick: move |_| back.call(()),
                                icons::ArrowLeft { class: "h-5 w-5" }
                            }
                        }
                        TitleBlock { title, subtitle }
                    }
                    // One right-aligned group: extra actions sit next to
                    // the add button, never floating mid-header.
                    div { class: "flex items-center gap-2 flex-wrap",
                        if let Some(actions) = actions {
                            {actions}
                        }
                        if can_write {
                            // Contrat documenté : bouton « ajouter » rendu ssi
                            // `on_add` ET `add_label` sont fournis (add_label None =
                            // page en mode détail, pas de création ici).
                            if let (Some(add), Some(label)) = (on_add, add_label) {
                                button {
                                    class: PRIMARY_BTN,
                                    r#type: "button",
                                    onclick: move |_| add.call(()),
                                    icons::Plus { class: "h-4 w-4 inline mr-1" }
                                    {label}
                                }
                            }
                        }
                    }
                }
            }
            if let Some(filters) = filters {
                div { class: "mb-4 md:mb-6 flex flex-wrap items-center gap-2", {filters} }
            }
            {children}
        }
    }
}

/// Bloc titre + sous-titre du header.
#[component]
fn TitleBlock(title: String, subtitle: Option<String>) -> Element {
    rsx! {
        div {
            h1 { class: "text-2xl md:text-3xl font-bold text-gray-900", {title} }
            if let Some(sub) = subtitle {
                p { class: "text-sm md:text-base text-gray-600 mt-1 md:mt-2", {sub} }
            }
        }
    }
}
