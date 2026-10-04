//! Coquille d'éditeur unifiée — le chrome partagé des trois éditeurs
//! (flows, dashboards, studio) : barre du haut en 3 zones, bandeau,
//! corps plein écran (canvas + palette flottante + inspecteur divulgatif),
//! invitation d'état vide. Les éditeurs restent propriétaires de leur état
//! (`EditorCx` + reducers) et de leur canvas ; ils passent ici des slots
//! `Element` (même école que `ListLayout` : props « prouvées », i18n du
//! chrome résolu en interne, le contenu vient de l'appelant).
//!
//! Principes (maquette validée 2026-09) :
//! 1. barre unique — gauche : retour + nom + statut · droite : version + actions ;
//! 2. statut uniforme — `StatusChip` point + libellé, chip version neutre `v{n}` ;
//! 3. palette à la demande — bouton `+` flottant → `PalettePopover` ;
//! 4. inspecteur seulement sur sélection — `InspectorPanel`, absent sinon ;
//! 5. état vide = invitation — « Cliquez sur + pour commencer ».

use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::components::icons;

pub mod floating_panel;
pub mod inspector;
pub mod palette;

pub use inspector::InspectorPanel;
pub use palette::{PaletteIcon, PaletteItem, PalettePopover};

/// Tonalité d'une chip — pilule + point, classes littérales complètes
/// (exigence du scan Tailwind : jamais de classe construite).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StatusTone {
    Green,
    Amber,
    Red,
    Slate,
    Purple,
}

impl StatusTone {
    /// Classes de la pilule (bordure/fond/texte), tonalité par tonalité.
    fn pill(self) -> &'static str {
        match self {
            StatusTone::Green => "border-green-200 bg-green-50 text-green-700",
            StatusTone::Amber => "border-amber-200 bg-amber-50 text-amber-700",
            StatusTone::Red => "border-red-200 bg-red-50 text-red-700",
            StatusTone::Slate => "border-gray-200 bg-gray-50 text-gray-600",
            StatusTone::Purple => "border-purple-200 bg-purple-50 text-purple-700",
        }
    }

    /// Classes du point coloré.
    fn dot(self) -> &'static str {
        match self {
            StatusTone::Green => "bg-green-500",
            StatusTone::Amber => "bg-amber-500",
            StatusTone::Red => "bg-red-500",
            StatusTone::Slate => "bg-gray-400",
            StatusTone::Purple => "bg-purple-500",
        }
    }
}

/// Statut affiché dans la barre — le même langage pour les trois éditeurs :
/// un point coloré + un libellé (+ infobulle d'erreur éventuelle).
#[derive(Clone, PartialEq, Debug)]
pub struct EditorStatus {
    pub tone: StatusTone,
    pub label: String,
    pub tooltip: Option<String>,
}

impl EditorStatus {
    pub fn new(tone: StatusTone, label: impl Into<String>) -> Self {
        Self {
            tone,
            label: label.into(),
            tooltip: None,
        }
    }

    pub fn with_tooltip(mut self, tooltip: impl Into<String>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }
}

/// Pilule de statut — point coloré + libellé (principe 2).
#[component]
pub fn StatusChip(status: EditorStatus) -> Element {
    let pill = format!(
        "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium {}",
        status.tone.pill()
    );
    let dot = format!("h-2 w-2 rounded-full {}", status.tone.dot());
    rsx! {
        span { class: "{pill}", title: status.tooltip.unwrap_or_default(),
            span { class: "{dot}" }
            {status.label}
        }
    }
}

/// Pilule annexe (modifications non enregistrées, version ancienne chargée…)
/// — même langage visuel que `StatusChip`, `on_remove` ajoute la croix.
#[component]
pub fn Chip(
    tone: StatusTone,
    label: String,
    #[props(default)] on_remove: Option<Callback<()>>,
) -> Element {
    let pill = format!(
        "inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-xs font-medium {}",
        tone.pill()
    );
    let dot = format!("h-1.5 w-1.5 rounded-full {}", tone.dot());
    rsx! {
        span { class: "{pill}",
            span { class: "{dot}" }
            {label}
            if let Some(cb) = on_remove {
                button {
                    class: "ml-0.5 opacity-70 hover:opacity-100",
                    onclick: move |_| cb.call(()),
                    icons::X { class: "h-3 w-3" }
                }
            }
        }
    }
}

/// La coquille — voir la doc du module pour le contrat.
#[component]
pub fn EditorShell(
    /// Retour vers la liste parente.
    on_back: Callback<()>,
    /// Nom de l'élément édité.
    title: String,
    /// Référence annexe (« #12 ») — None tant que le détail n'est pas chargé.
    #[props(default)]
    subtitle: Option<String>,
    /// Statut courant (point + libellé).
    status: EditorStatus,
    /// Version courante — chip neutre « v{n} » (rendue ici, zéro i18n).
    #[props(default)]
    version: Option<i64>,
    /// Chips annexes (dirty, version ancienne chargée…).
    #[props(default)]
    extra_chips: Option<Element>,
    /// Zone droite : boutons d'action de l'éditeur (enregistrer, déployer…).
    actions: Element,
    /// Bandeau sous la barre (violations, encart « Déployer ? », read-only).
    #[props(default)]
    banner: Option<Element>,
    /// Canvas — rendu dans `div.absolute.inset-0` : la racine du composant
    /// canvas doit se dimensionner sur son parent (`h-full w-full`).
    canvas: Element,
    /// Bouton `+` flottant + popover de palette (`PalettePopover`).
    #[props(default)]
    palette: Option<Element>,
    /// Pill d'outils flottante à côté du `+` (outils de mode, undo/redo…).
    #[props(default)]
    tools: Option<Element>,
    /// Inspecteur (`InspectorPanel`) — None ⇒ absent de l'écran (principe 4).
    #[props(default)]
    inspector: Option<Element>,
    /// Invitation d'état vide — Some ⇒ voile centré « Cliquez sur + … ».
    #[props(default)]
    empty_hint: Option<String>,
    /// Renommage inline — Some ⇒ le titre devient éditable au clic
    /// (Entrée / blur valident, Échap annule). Réservé aux utilisateurs
    /// autorisés (l'éditeur ne le passe que si `can_write`).
    #[props(default)]
    on_rename: Option<Callback<String>>,
) -> Element {
    let mut renaming = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let rename_title = t!("eshell-rename");
    // Deux closures (`onkeydown`/`onblur`) consomment chacune leur copie
    // (un String ne peut être déplacé dans deux closures `move`).
    // Hide the mobile app header while this editor is mounted (see
    // `ui::EDITORS_OPEN`); the effect reads no signal, so it runs once.
    use_effect(|| {
        *crate::state::ui::EDITORS_OPEN.write() += 1;
    });
    use_drop(|| {
        let open = *crate::state::ui::EDITORS_OPEN.peek();
        *crate::state::ui::EDITORS_OPEN.write() = open.saturating_sub(1);
    });
    let title_enter = title.clone();
    let title_blur = title.clone();
    rsx! {
        // Full height: the mobile app header is hidden while an editor is
        // open (`ui::EDITORS_OPEN`); desktop content column already carries
        // the sidebar offset (lg:pl-64/16).
        div { class: "flex h-[100dvh] flex-col lg:h-screen",
            // ─── Zone 1/2 : barre du haut (gauche identité · droite version+actions)
            // Phone: two rows (identity, then actions right-aligned) instead
            // of one 680 px row scrolled sideways; single 56 px row from sm up.
            div { class: "flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1.5 bg-white border-b border-gray-200 px-3 py-2 sm:h-14 sm:flex-nowrap sm:overflow-x-auto sm:py-0",
                button {
                    class: "inline-flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm text-gray-600 hover:bg-gray-100",
                    onclick: move |_| on_back.call(()),
                    icons::ArrowLeft { class: "h-4 w-4" }
                    span { class: "hidden sm:inline", {t!("eshell-back")} }
                }
                div { class: "h-5 w-px shrink-0 bg-gray-200" }
                // Titre : plain quand pas de renommage ; sinon clic → input
                // inline (Entrée/blur valident, Échap annule, vide ignoré).
                {
                    match on_rename {
                        Some(cb) if renaming() => rsx! {
                            input {
                                class: "w-56 shrink-0 rounded-lg border border-blue-400 px-2 py-1 text-sm font-semibold text-gray-900 focus:outline-none focus:ring-2 focus:ring-blue-500",
                                value: "{draft}",
                                autofocus: true,
                                oninput: move |e: Event<FormData>| draft.set(e.value()),
                                onkeydown: move |e: KeyboardEvent| {
                                    if e.key() == Key::Escape {
                                        renaming.set(false);
                                    } else if e.key() == Key::Enter {
                                        renaming.set(false);
                                        let value = draft().trim().to_string();
                                        if !value.is_empty() && value != title_enter {
                                            cb.call(value);
                                        }
                                    }
                                },
                                onblur: move |_| {
                                    renaming.set(false);
                                    let value = draft().trim().to_string();
                                    if !value.is_empty() && value != title_blur {
                                        cb.call(value);
                                    }
                                },
                            }
                        },
                        Some(_) => rsx! {
                            button {
                                class: "group -ml-1 flex min-w-0 items-center gap-1 rounded-lg px-1.5 py-0.5 hover:bg-gray-100 sm:shrink-0",
                                title: "{rename_title}",
                                onclick: move |_| {
                                    draft.set(title.clone());
                                    renaming.set(true);
                                },
                                span { class: "min-w-0 max-w-[15rem] truncate text-base font-semibold text-gray-900",
                                    {title.clone()}
                                }
                                icons::Pencil { class: "h-3.5 w-3.5 shrink-0 text-gray-400 group-hover:text-gray-600" }
                            }
                        },
                        None => rsx! {
                            h2 { class: "min-w-0 max-w-[16rem] truncate text-base font-semibold text-gray-900 sm:shrink-0",
                                {title}
                            }
                        },
                    }
                }
                if let Some(sub) = subtitle {
                    span { class: "hidden shrink-0 text-xs text-gray-400 sm:inline",
                        {sub}
                    }
                }
                StatusChip { status }
                // Chips annexes (dirty, version ancienne…)
                {extra_chips}
                div { class: "hidden min-w-4 flex-1 sm:block" }
                if let Some(v) = version {
                    span { class: "hidden shrink-0 items-center rounded-full border border-gray-200 bg-gray-50 px-2.5 py-0.5 text-xs font-medium text-gray-600 sm:inline-flex",
                        {format!("v{v}")}
                    }
                }
                // Zone 3 : actions de l'éditeur (boutons existants, déplacés).
                div { class: "flex w-full items-center justify-end gap-2 overflow-x-auto sm:w-auto sm:shrink-0 sm:overflow-visible",
                    {actions}
                }
            }
            // ─── Bandeau (violations / read-only / encart déploiement)
            if let Some(b) = banner {
                div { class: "shrink-0 space-y-2 px-3 pt-2", {b} }
            }
            // ─── Corps : canvas plein écran + overlays à la demande
            div { class: "relative min-h-0 flex-1",
                div { class: "absolute inset-0", {canvas} }
                if let Some(hint) = empty_hint {
                    div { class: "pointer-events-none absolute inset-0 z-10 flex items-center justify-center",
                        p { class: "rounded-lg bg-white/80 px-4 py-2 text-sm text-gray-400",
                            {hint}
                        }
                    }
                }
                if let Some(p) = palette {
                    div { class: "absolute left-4 top-4 z-20", {p} }
                }
                if let Some(t) = tools {
                    div { class: "absolute left-[4.5rem] top-4 z-20", {t} }
                }
                {inspector}
            }
        }
    }
}
