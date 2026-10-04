//! Table générique du socle CRUD — colonnes déclaratives, rendu de cellule
//! custom, markup exact des tables historiques (tokens `.th`/`.td`).
//! Ni tri ni pagination ici : le tri n'existe dans aucune page (point
//! d'extension V2), la pagination compose avec le `Pager` existant dans le
//! corps de page. Les états sont délégués à `ListStates`.

use std::collections::HashSet;
use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_i18n::t;

/// Clé de ligne — newtype autour d'une closure `&T -> String` : la macro
/// `#[component]` génère un `PartialEq` par champ sur les props, et
/// `Rc<dyn Fn>` n'implémente pas ce trait (comparaison par pointeur ici,
/// même traitement que [`Column`]).
pub struct RowKey<T: 'static>(Rc<dyn Fn(&T) -> String>);

impl<T: 'static> RowKey<T> {
    /// Clé de ligne, ex. `RowKey::new(|f: &FlowSummary| f.id.to_string())`.
    pub fn new(key: impl Fn(&T) -> String + 'static) -> Self {
        Self(Rc::new(key))
    }
}

impl<T: 'static> Clone for RowKey<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: 'static> PartialEq for RowKey<T> {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

/// Colonne déclarative d'une [`DataTable`] — le rendu de cellule couvre le
/// **contenu** du `<td>` : la table fournit la balise, le token `td` et les
/// classes additionnelles.
#[derive(Clone)]
pub struct Column<T: 'static> {
    /// En-tête (i18n résolu par l'appelant).
    pub header: String,
    /// Classes additionnelles du `<td>` (ex. « font-medium text-gray-900 »).
    pub td_class: &'static str,
    /// Rendu du contenu de cellule pour une ligne (boutons, badges…).
    pub cell: Rc<dyn Fn(&T) -> Element>,
    /// Mobile behaviour of the column (desktop rendering is unchanged).
    pub kind: ColumnKind,
}

/// Mobile behaviour of a [`Column`]. Below the `md` breakpoint the table
/// scrolls horizontally; these roles keep it usable on a phone.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ColumnKind {
    /// Always visible.
    #[default]
    Normal,
    /// Hidden below `md` (dates, versions, details) so the essential
    /// columns fit a phone screen.
    Secondary,
    /// Row actions: pinned to the right edge below `md`, so they stay
    /// reachable while the rest of the row scrolls.
    Actions,
}

// Full class literals (Tailwind scans the sources, no string building).
// Public so hand-written tables (outside `DataTable`) behave the same on phones.
const SECONDARY_CLASS: &str = "hidden md:table-cell";
pub const ACTIONS_TH_CLASS: &str = "sticky right-0 bg-gray-50 md:static";
pub const ACTIONS_TD_CLASS: &str = "sticky right-0 bg-white group-hover:bg-gray-50 whitespace-nowrap shadow-[-6px_0_6px_-6px_rgba(0,0,0,0.25)] md:static md:shadow-none";

impl ColumnKind {
    fn th_class(self) -> &'static str {
        match self {
            ColumnKind::Normal => "",
            ColumnKind::Secondary => SECONDARY_CLASS,
            ColumnKind::Actions => ACTIONS_TH_CLASS,
        }
    }

    fn td_class(self) -> &'static str {
        match self {
            ColumnKind::Normal => "",
            ColumnKind::Secondary => SECONDARY_CLASS,
            ColumnKind::Actions => ACTIONS_TD_CLASS,
        }
    }
}

impl<T: 'static> Column<T> {
    pub fn new(header: String, cell: impl Fn(&T) -> Element + 'static) -> Self {
        Self {
            header,
            td_class: "",
            cell: Rc::new(cell),
            kind: ColumnKind::Normal,
        }
    }

    /// Pose les classes additionnelles du `<td>`.
    pub fn with_td_class(mut self, td_class: &'static str) -> Self {
        self.td_class = td_class;
        self
    }

    /// Hides the column below `md` (see [`ColumnKind::Secondary`]).
    pub fn secondary(mut self) -> Self {
        self.kind = ColumnKind::Secondary;
        self
    }

    /// Marks the row-actions column (see [`ColumnKind::Actions`]).
    pub fn actions(mut self) -> Self {
        self.kind = ColumnKind::Actions;
        self
    }
}

// `PartialEq` exigé par les props des composants dioxus (généré par la
// macro) : structurel sur les libellés/classes, par pointeur sur la closure
// (une nouvelle `Rc` à chaque rendu page → jamais mémoïsé, identique au
// rendu inline historique).
impl<T> PartialEq for Column<T> {
    fn eq(&self, other: &Self) -> bool {
        self.header == other.header
            && self.td_class == other.td_class
            && self.kind == other.kind
            && Rc::ptr_eq(&self.cell, &other.cell)
    }
}

#[component]
pub fn DataTable<T: Clone + PartialEq + 'static>(
    /// Colonnes, dans l'ordre du rendu.
    columns: Vec<Column<T>>,
    /// Lignes de la page courante (le filtrage/pagination restent côté page).
    rows: Vec<T>,
    /// Clé de ligne (attribut `key` du `<tr>`, ex. `|f| f.id.to_string()`).
    row_key: RowKey<T>,
    /// Clic sur une ligne (reçoit la clé de ligne) — ex. ouvrir un détail.
    #[props(default)]
    on_row_click: Option<Callback<String>>,
    /// Optional row selection, keyed by the row key: when set, a leading
    /// checkbox column appears (header checkbox = select/clear all visible
    /// rows). `None` = no selection column (default).
    #[props(default)]
    selected: Option<Signal<HashSet<String>>>,
) -> Element {
    // Clés pré-calculées hors rsx (pas de `let` dans le rsx ; la macro
    // d'attribut exige en outre un `PartialEq` sur chaque champ de props,
    // d'où le newtype RowKey).
    let keyed_rows: Vec<(String, &T)> = rows.iter().map(|row| ((row_key.0)(row), row)).collect();
    let row_click = on_row_click;
    // Selection state of the visible rows (header checkbox).
    let visible_keys: Vec<String> = keyed_rows.iter().map(|(k, _)| k.clone()).collect();
    let all_selected = selected.is_some_and(|sel| {
        let sel = sel.read();
        !visible_keys.is_empty() && visible_keys.iter().all(|k| sel.contains(k))
    });

    rsx! {
        // Horizontal scroll instead of clipping: on a phone the columns past
        // the viewport (row actions first) must stay reachable.
        div { class: "bg-white rounded-lg shadow-sm overflow-x-auto",
            table { class: "min-w-full divide-y divide-gray-200",
                thead { class: "bg-gray-50",
                    tr {
                        if let Some(mut sel) = selected {
                            th { class: "th w-8",
                                input {
                                    r#type: "checkbox",
                                    aria_label: t!("crud-select-all"),
                                    checked: all_selected,
                                    onclick: move |event| event.stop_propagation(),
                                    onchange: move |_| {
                                        let keys = visible_keys.clone();
                                        sel.with_mut(|set| {
                                            if all_selected {
                                                for k in &keys {
                                                    set.remove(k);
                                                }
                                            } else {
                                                set.extend(keys);
                                            }
                                        });
                                    },
                                }
                            }
                        }
                        for col in columns.iter() {
                            th { class: "th {col.kind.th_class()}", {col.header.clone()} }
                        }
                    }
                }
                tbody { class: "bg-white divide-y divide-gray-200",
                    for (key, sel_key, row) in keyed_rows.iter().map(|(k, r)| (k.clone(), k.clone(), *r)) {
                        tr {
                            key: "{key}",
                            class: if row_click.is_some() { "group hover:bg-gray-50 cursor-pointer" } else { "group hover:bg-gray-50" },
                            onclick: move |_| {
                                if let Some(cb) = row_click.as_ref() {
                                    cb.call(key.clone());
                                }
                            },
                            if let Some(mut sel) = selected {
                                td { class: "td w-8",
                                    input {
                                        r#type: "checkbox",
                                        aria_label: t!("crud-select-row"),
                                        checked: sel.read().contains(&sel_key),
                                        onclick: move |event| event.stop_propagation(),
                                        onchange: move |_| {
                                            let key = sel_key.clone();
                                            sel.with_mut(|set| {
                                                if !set.remove(&key) {
                                                    set.insert(key);
                                                }
                                            });
                                        },
                                    }
                                }
                            }
                            for col in columns.iter() {
                                td { class: "td {col.kind.td_class()} {col.td_class}",
                                    {(col.cell)(row)}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
