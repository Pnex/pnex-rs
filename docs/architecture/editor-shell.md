# Coquille d'éditeur unifiée (`EditorShell`)

> Décision 2026-09-18 — unification du chrome des trois éditeurs (flows,
> dashboards, tours) derrière une brique Dioxus partagée, d'après une
> maquette validée. Les éditeurs gardent leur page, leur état (`EditorCx`)
> et leur canvas ; seul le chrome est mutualisé.

## Composants (`src/components/editor_shell/`)

| Composant | Rôle |
|---|---|
| `EditorShell` | Barre 3 zones (retour · nom · statut ‖ version · actions), bandeau (`banner`), corps plein écran : canvas dans `div.absolute.inset-0`, slots flottants palette (`left-4 top-4`), outils (`left-[4.5rem] top-4`), inspecteur (`right-0`), invitation d'état vide. |
| `StatusChip` / `Chip` | Langage de statut unique : point coloré + libellé (`StatusTone` Green/Amber/Red/Slate/Purple), chip version neutre `v{n}` rendue par le shell, chips annexes (dirty, version ancienne `on_remove`). |
| `PalettePopover` | Palette à la demande : bouton `+` → popover avec recherche ; items data plate (`PaletteItem` : clé, label, icône `PaletteIcon`, tuile). **Sans backdrop** — le drag d'un modèle (D41) doit voir ses `pointermove/up` atteindre le canvas ; fermeture = pick / croix / Échap / re-clic `+`. |
| `InspectorPanel` | Inspecteur divulgatif : absent sans sélection (principe 4), `absolute inset-y-0 right-0 w-80`, fermeture croix/Échap (bubbling du formulaire). Le corps du formulaire reste à l'éditeur (`InspectorBody`). |
| `FloatingPanel` | Panneau flottant annexe (étages studio : liste, mode liaison, compteur), dans le slot `tools`. |

## Contrat des éditeurs

- **Slots `Element`** (école `ListLayout`) : `palette`, `tools`, `inspector`,
  `banner`, `extra_chips`, `actions` — pas de struct de config en prop
  (`#[component]` dérive `PartialEq` par champ ; `Element`/`Callback` sont
  « prouvées »).
- **Pages hôtes** : l'éditeur est monté **hors** `ListLayout` (`flows.rs`,
  `studio.rs`) — plus de bouton « + Nouveau » en contexte d'édition.
- **Création immédiate** (flows, dashboards, **tours**) : pas de modale —
  « + Nouveau » POST directement avec un nom daté (`flows-default-name` /
  `db-default-name` / `studio-default-name`, heure locale via
  `util::now_label()`, `Date` JS sur wasm) et ouvre l'éditeur sur l'élément
  créé.
- **Renommage inline** : quand l'éditeur passe `on_rename` (si `can_write`),
  le titre de la coquille devient éditable au clic (crayon, Entrée/blur
  valident, Échap annule). Le renommage réutilise le PATCH de sauvegarde
  (les 3 `Update*` voyagent avec le contenu) : **renommer vaut sauvegarde
  de l'état courant** et crée une version — pour un dashboard, c'est une
  publication (D24).
- **Canvas** : racine `relative h-full w-full` (flow/studio) ou équivalent
  (dashboard) dans le wrapper `absolute inset-0` ; les
  `h-[calc(100vh-16rem)]` / `-9rem` disparaissent ; coquille
  `h-[calc(100dvh-4rem)] lg:h-screen`.
- **Statuts unifiés** : flow = fusion statut DB + santé runtime
  (Erreur rouge / Déployé vert / Déployé · à redéployer ambre / Arrêté et
  Brouillon gris ; infobulle `pid · vN — last_error`) ; dashboard = `Live` ;
  studio = `Publié · vN` / `Brouillon`. Le chip « Moteur actif · v12 » et
  les badges par éditeur sont absorbés.
- **Échap** : hiérarchie locale sans listener global — input de recherche
  (ferme le popover) / canvas (désélectionne, ajouté aux canvas flow et
  studio) / panneau inspecteur (ferme, bubbling des champs).
- **i18n** : clés `eshell-*` (chrome) ; libellés de contenu résolus par les
  éditeurs (`flows-palette-*`, `lib-kind-*`, `studio-add-*`).

## Sidebar en rail d'icônes

`state::ui::RAIL` (`GlobalSignal<bool>` persisté `pnex.sidebar_rail`,
restauré au montage du shell) : sidebar desktop `lg:w-64 ⇄ lg:w-16`,
contenu `lg:pl-64 ⇄ lg:pl-16`, classes **littérales complètes** via `match`
(exigence scan Tailwind). Rail : icônes seules centrées + infobulle `title`,
groupes nav = clic rouvre la sidebar sur le groupe (enfants jamais rendus en
rail), brand = icône, pied = toggle + déconnexion. Drawer mobile inchangé.
