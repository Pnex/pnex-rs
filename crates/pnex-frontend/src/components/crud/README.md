# Socle CRUD (listes)

Motif « liste d'une ressource » écrit **une seule fois** — unifier les ~10
pages qui dupliquaient header / retour / ajouter / filtres / états avec
dérive progressive. Créer une nouvelle page liste se réduit à composer les
blocs ci-dessous, sans jamais réécrire la coquille.

## Composants

| Composant | Rôle | Points clés |
|---|---|---|
| `layout::ListLayout` | Coquille : header (titre, sous-titre, retour, « ajouter », slot `actions`), slot filtres, corps | `on_back` → bouton borduré icône `h-5 w-5` (agrandi) ; slot `actions` pour les en-têtes multi-boutons (media) ; bouton ajouter rendu ssi `can_write && on_add`+`add_label` |
| `states::ListStates` | Zones loading / empty / error | `state: Option<Result<(), ApiError>>` — **non générique** ; empty enrichi via `empty_icon`/`empty_detail` (catalog, media) |
| `table::DataTable<T>` / `table::Column<T>` | Table (tokens `.th`/`.td`, markup historique) | Colonnes déclaratives, cellule = `Fn(&T) -> Element` **sans** le `<td>` ; `on_row_click` (reçoit la clé de ligne — annotations) ; pas de tri (V2), pagination hors table |
| `filters::FilterBar` / `SearchInput` / `RefreshButton` | Barre de filtres | Search : saisie **live** (refetch à la frappe quand le signal est lu en synchrone de la resource), Enter → `on_submit` (reset page 0 côté page) ; `RefreshButton` icône seule partout (un seul idiome, tooltip i18n) |
| `pager::ListPager` | Pagination | Délègue au `Pager` existant (D14) ; câblage `page.set` intégré, `page_size` défaut 10 (`PAGE_SIZE`) |
| `form::FormDialog` | Modal création/édition | `Modal` + pied cancel/submit (`busy`, `valid`) ; champs et signaux restent à la page |

Réutilisés tels quels : `Modal`, `ConfirmDialog`,
`LoadingOverlay`, `toasts`, `icons`, `badges`.

## Exemple minimal

```rust
use dioxus::prelude::*;
use dioxus_i18n::t;

use crate::api;
use crate::components::crud::filters::{FilterBar, RefreshButton, SearchInput};
use crate::components::crud::form::FormDialog;
use crate::components::crud::layout::ListLayout;
use crate::components::crud::pager::{ListPager, PAGE_SIZE};
use crate::components::crud::states::ListStates;
use crate::components::crud::table::{Column, DataTable, RowKey};

#[component]
pub fn Things() -> Element {
    let mut reload = use_signal(|| 0u32);
    let mut search = use_signal(String::new);
    let mut page = use_signal(|| 0i64);

    let list = use_resource(move || {
        // Filtres construits dans la partie SYNCHRONE de la closure
        // (abonnement garanti aux signaux), fetch ensuite.
        let filters = api::things::ThingFilters {
            search: { let v = search().trim().to_string(); (!v.is_empty()).then_some(v) },
            limit: Some(PAGE_SIZE),
            offset: Some(page() * PAGE_SIZE),
        };
        async move {
            let _ = reload();
            api::things::list(&filters).await
        }
    });

    // Lecture synchrone de la ressource → discriminant + données.
    let (state, is_empty, count, rows) = match &*list.value().read() {
        None => (None, false, 0, Vec::new()),
        Some(Ok(paged)) => (
            Some(Ok(())),
            paged.results.is_empty() && paged.count == 0,
            paged.count,
            paged.results.clone(),
        ),
        Some(Err(err)) => (Some(Err(err.clone())), false, 0, Vec::new()),
    };

    let columns = vec![
        Column::new(t!("things-col-name").to_string(), |t: &api::things::Thing| {
            rsx! { "{t.name}" }
        })
        .with_td_class("font-medium text-gray-900"),
        // … colonne actions : boutons capturant les signaux (Copy) de la page.
    ];

    rsx! {
        ListLayout {
            title: t!("nav-things").to_string(),
            subtitle: Some(t!("things-subtitle").to_string()),
            can_write: true,
            add_label: Some(t!("things-new").to_string()),
            on_add: move |_| /* ouvrir FormDialog */,
            div { class: "space-y-0",
                FilterBar {
                    SearchInput {
                        placeholder: t!("things-search-placeholder").to_string(),
                        value: search,
                        on_submit: move |_| {
                            page.set(0);
                            reload.with_mut(|r| *r += 1);
                        },
                    }
                    RefreshButton { on_click: move |_| reload.with_mut(|r| *r += 1) }
                }
                ListStates {
                    state: state,
                    is_empty: is_empty,
                    empty_message: t!("things-empty").to_string(),
                    div { class: "relative",
                        DataTable {
                            columns: columns,
                            rows: rows.clone(),
                            row_key: RowKey::new(|t: &api::things::Thing| t.id.to_string()),
                        }
                        ListPager { count: count, page: page }
                    }
                }
            }
        }
    }
}
```

## Règles

1. **i18n côté appelant** : le socle ne prend que des `String` — le `t!`
   reste dans la page (une clé manquante = panic, garde registry↔locales).
2. **Garde org hors socle** : `org::current().is_none()` → message
   « orgs-empty », en tête du `children` de `ListLayout`.
3. **`can_write` calculé par la page** (helper `current_role()` privé par
   page — convention projet) ; le socle ne fait que masquer les actions.
4. **`derive PartialEq` sur les types payload** passés aux props du socle
   (`DataTable<T>`, colonnes…) : la macro `#[component]` de dioxus 0.7
   génère un `PartialEq` par champ sur les props. Précédents : `ApiError`,
   `FlowSummary`, types `notify.rs`.
5. **Chargement** : `use_resource` + compteur `reload` + filtres lus dans la
   partie **synchrone** de la closure (piège dioxus : capture plate ≠ dep
   trackée). Nouveau filtre ⇒ `page.set(0)` puis reload.
6. **Pagination** : `ListPager` dans le corps (jamais dans `DataTable`) —
   taille de page unique par défaut (`PAGE_SIZE`, D14 : 10), câblage
   `page.set` intégré (surcharge `page_size` pour les grilles denses).

## Checklist de migration d'une page

- [ ] Header → `ListLayout` (garder les props i18n existantes).
- [ ] Filtres → `FilterBar` + `SearchInput`/`RefreshButton` ; selects
      spécifiques restent en rsx page dans la barre.
- [ ] Match d'états → calcul synchrone + `ListStates`.
- [ ] Table → `DataTable` + `Column` (une colonne actions pour les
      boutons de ligne) ; `ListPager` pour la pagination.
- [ ] Modal de création/édition → `FormDialog` (attention : `valid: false`
      si la validation s'affiche après clic).
- [ ] Types payload : `derive PartialEq`.
- [ ] Iso-fonctionnalité : Enter/recharge, gating viewer, toasts, refetch
      après create/delete/retour d'éditeur.

## Non inclus (V2 envisagée)

Tri de table, `FilterSelect` générique (revisitée à la migration devices,
3 selects), bouton « réessayer » sur erreur (`on_retry` + clé
`common-retry`), slot `submit_icon` dans `FormDialog`, composant `Button`
global.
