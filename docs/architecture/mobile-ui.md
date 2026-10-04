# UI mobile — audit d'affichage et plan de correction

> Audit fait le 2026-10-04 sur le téléphone de l'user (Xiaomi 21091116UG,
> 1080×2400, DPR 2,75 → **viewport CSS 392×796**), APK installée,
> pilotage de la WebView par CDP (`adb forward` sur
> `webview_devtools_remote_<pid>`, Playwright `connectOverCDP`) : navigation
> par les liens de la sidebar, capture d'écran + mesure automatique des
> débordements (`scrollWidth > clientWidth`, éléments dont le bord droit
> dépasse 392 px). Org de l'user + org E2E (pour avoir des lignes : devices,
> événements). Tout est mesuré, rien n'est supposé.
>
> Périmètre de ce document : **les pages CRUD d'abord** (lot 1–3), puis la
> barre d'outils des éditeurs (lot 4) ; le reste (carte, visualisation,
> modales) est consigné, traité plus tard.

## 1. Anomalies constatées

Gravité : **B** = bloquant (fonction inaccessible au doigt), **M** = majeur
(lisibilité / glisser obligatoire), **m** = mineur (finition).

### Tables CRUD (socle `components/crud/table.rs`, 17 pages)

| # | Grav. | Constat | Cause |
|---|---|---|---|
| M1 | **B** | Toutes les tables `DataTable` sont **coupées à droite sans défilement possible** : le doigt ne fait rien, les colonnes au-delà de ~345 px n'existent pas pour l'user. Mesuré : dashboards 345<593, flows 344<557, catalogue 344<573, orgs 344<518, devices 344<782. | wrapper `div.bg-white.rounded-lg.shadow-sm.overflow-hidden` autour de `table.min-w-full` : `overflow-hidden` coupe au lieu de défiler. |
| M2 | **B** | Conséquence directe : **la colonne Actions est invisible** partout. Devices → « Détail » / « Recompiler » hors écran (impossible d'ouvrir un device), Orgs → « Gérer » / « Définir active » hors écran, Flows → « Ouvrir » / « Supprimer », Dashboards → seul un liseré du bouton éclair dépasse, Catalogue → « Voir le pinout ». | M1 + actions en dernière colonne, `whitespace-nowrap`. |
| M3 | M | Lignes démesurées : devices **196 px de haut par ligne** (colonne Firmware = statut + date + build en service + dernier build + lien), noms éclatés sur 3–4 lignes (« Flow / 2026- / 10-04 / 09:22 », « Dashboard / 2026-10- / 04 11:25 »). Deux devices remplissent l'écran. | Colonnes trop nombreuses (4 à 7, notifications jusqu'à 8) se partageant 345 px ; la colonne nom est la plus comprimée. |
| M4 | M | Padding de cellule desktop sur mobile : `.th` / `.td` = `px-6` (24 px × 2 par colonne) → sur 7 colonnes, ~340 px de padding seul, soit la largeur de l'écran. | tokens `.th`/`.td` de `style/tailwind.css` sans variante mobile. |
| M5 | m | En-têtes coupés en deux lignes (« MISE À / JOUR »), colonne de dates tronquée (« 09:22:4( »). | pas de `whitespace-nowrap` sur `th`. |
| M6 | M | Tables hors socle (`events`, `models`, `notify_deliveries`, `llm_providers`, `admin_status`, `agent_panel`) : elles **défilent** (`overflow-x-auto`) mais sans aucun indice visuel ; message d'événement écrasé sur 4–5 lignes dans une colonne de 86 px. | table brute, pas de priorisation de colonnes. |

### En-tête et barre de filtres des pages liste (`ListLayout`)

| # | Grav. | Constat | Cause |
|---|---|---|---|
| M7 | m | Bouton Rafraîchir placé de 4 façons différentes selon la page : à côté de « Nouveau » (dashboards, models, controls), seul en haut à droite (cameras, orgs), au bout de la recherche (functions, secrets, studio…), seul sur une 2e ligne (flows). | chaque page pose son `RefreshButton` dans un slot différent (`actions` vs `filters`). |
| M8 | m | Filtres qui se replient au hasard : flows = select + recherche puis le refresh seul sur une ligne ; devices = 3 selects sur 2 lignes puis la recherche ; catalogue = placeholder tronqué (« Rechercher (nom, description, carte, ‹ »). | `flex-wrap` sans largeur cible ; la recherche n'est pas `w-full` en mobile. |
| M9 | m | Marges desktop : `p-6` de page + `mb-8` sous le header + titre `text-3xl` + sous-titre long → la 1re ligne de table arrive à ~40 % de l'écran (dashboards : y≈630/796 px CSS pour la 1re ligne). | `ListLayout` sans variantes `sm:`. |
| M10 | m | Le bouton flottant de l'assistant (💬, bas-droite) **recouvre le contenu** en fin de page : dernière cellule de table, select Langue du profil, valeur « Stockage des index » d'admin/status (« 323. » masqué). | `main` sans `padding-bottom` réservé au FAB. |

### Barre d'outils des éditeurs (`components/editor_shell/mod.rs`)

| # | Grav. | Constat | Cause |
|---|---|---|---|
| M11 | **M** | « La ligne de boutons en haut est trop large, il faut glisser » : barre flow = 680 px pour 392 (Enregistrer à 467, Déployer à 563, Historique à 668) ; barre dashboard = 613 px (Enregistrer, Historique hors écran). Aucun indice qu'il faut glisser : **Enregistrer et Déployer sont invisibles à l'ouverture**. | `div.flex.h-14…overflow-x-auto` : titre + crayon + `#17` + badge statut + badge version + boutons texte, tous `shrink-0`. |
| M12 | m | Double en-tête sur les éditeurs : header mobile de l'app (logo, 64 px) + barre éditeur (56 px) = 120 px de chrome sur 796. | Shell mobile toujours rendu. |

### Autres pages (consignées, hors lot CRUD)

| # | Grav. | Constat |
|---|---|---|
| M13 | M | Carte (`/map`) : le panneau de recherche POI occupe toute la largeur, la carte n'est visible que sur une bande de ~90 px ; le bouton « ＋ Ajouter un POI » déborde (bord droit 394 > 392) ; attribution OSM tronquée. |
| M14 | m | Visualisation : le libellé « Auto · 15 s » se replie en colonne de 3 lignes à côté du titre. Accueil : « Rafraîchissement / 15 s » coince le sous-titre sur 3 lignes. |
| M15 | m | Modale pinout (catalogue) : libellés de pins (~6 px CSS) illisibles, aucune possibilité de zoom. |
| M16 | m | Le drawer mobile n'a **pas la recherche globale** (D69 : `SidebarSearch` n'est rendu que dans la sidebar desktop). |
| M17 | m | Détail org : « Ajouter un fournisseur » replié sur 2 lignes à côté du paragraphe ; table des membres non vérifiée (une seule org mono-membre). |

Non vérifié faute de données (listes vides dans les deux orgs) : functions,
controls, secrets, studio, media, annotations, firmware, edges/refs,
mixtures, cameras, notifications (canaux/modèles). Toutes passent par
`DataTable` avec 4 à 8 colonnes → M1–M5 s'y appliquent par construction.

## 2. Plan de correction (CRUD d'abord)

Principe : corriger **dans le socle** (`components/crud/`), pas page par
page — 17 pages bénéficient de chaque lot. Desktop strictement inchangé
(toutes les modifs derrière des variantes `sm:`/`md:` ou des classes
mobiles-first qui retombent sur l'existant à partir de `md`).

### Lot 1 — rendre tout atteignable (corrige M1, M2, M5)

1. `DataTable` : wrapper `overflow-hidden` → `overflow-x-auto` (garder
   l'arrondi : `overflow-x-auto` clippe aussi les coins).
2. `th` en `whitespace-nowrap`.
3. Colonne actions **collante à droite** sur mobile (`sticky right-0
   bg-white` + ombre gauche) : les actions restent visibles même quand la
   table défile. Nécessite un marqueur de colonne → `Column::actions()`.
4. Indice de défilement : dégradé d'ombre sur le bord droit quand
   `scrollWidth > clientWidth` (CSS pur, `background-attachment: local`).

→ Petit diff, gain immédiat : plus rien d'inaccessible.

### Lot 2 — densité mobile (M3, M4, M9)

1. Tokens `.th`/`.td` : `px-3 py-2` en mobile, `md:px-6 md:py-3` (desktop
   identique).
2. Priorité de colonne dans le socle :
   `Column::hide_below(Breakpoint::Md)` → `hidden md:table-cell` sur `th`
   et `td`. Chaque page marque ses colonnes secondaires (dates, version,
   firmware détaillé, capacités…). Cible : **≤ 3 colonnes visibles + actions
   à 392 px**.
3. Colonne nom : `min-w-[8rem]` + `break-words` au lieu de casser à chaque
   tiret ; sous-ligne secondaire (date, id) gardée en `text-xs`.
4. Devices : colonne Firmware résumée en mobile (badge statut seul, détail
   dans la page device).
5. `ListLayout` : `p-4 md:p-6`, `mb-4 md:mb-8`, titre `text-2xl
   md:text-3xl`, sous-titre `line-clamp-2` en mobile.

### Lot 3 — cohérence en-tête / filtres / FAB (M7, M8, M10)

1. Une seule place pour Rafraîchir : slot dédié du `ListLayout`
   (`on_refresh`), rendu à droite du titre ; retrait des refresh posés à
   la main dans les filtres.
2. `FilterBar` mobile : recherche `w-full` en première ligne, selects en
   grille 2 colonnes dessous.
3. `main` : `pb-24` en mobile pour libérer le FAB assistant.

### Lot 4 — barre d'outils éditeurs (M11, M12)

1. Mobile : actions principales en **icône seule** (Enregistrer, Déployer,
   Historique — `aria-label` + `title` via `t!`), libellés dès `sm:`.
2. Titre `min-w-0 truncate` (il cède la place, plus les boutons) ; `#id` et
   badge version masqués en mobile (dans un menu « ⋯ » si utiles).
3. Retirer `overflow-x-auto` de la barre une fois qu'elle tient en 392 px.
4. Évaluer : masquer le header mobile de l'app sur les routes éditeur (le
   bouton retour de la barre suffit) → +64 px de canevas.

### Hors lot (plus tard)

M13 carte (panneau POI en bottom-sheet repliable), M14 en-têtes accueil /
visualisation, M15 pinout zoomable, M16 recherche globale dans le drawer,
M17 détail org, M6 tables hors socle (les migrer sur `DataTable` ou leur
appliquer les classes du lot 2).

### Vérification

- Rejouer la sonde CDP (même script : routes de la sidebar, mesure des
  débordements) après chaque lot : critère = **aucun `overflow-hidden`
  coupant, aucune action au-delà de 392 px**, captures avant/après.
- À terme : en faire un test `tests-android/` (harnais e2e Android) qui
  échoue si un bouton d'action sort du viewport.
- Desktop : passe Playwright web existante + capture de 2–3 listes à
  1280 px pour vérifier l'iso-rendu.

## 3. Avancement

- **2026-10-04 — lots 1, 2 (socle) et 4 livrés**, vérifiés sur le téléphone
  (sonde CDP rejouée) :
  - socle : `DataTable` défile (`overflow-x-auto`), rôles de colonne
    `Column::secondary()` (masquée sous `md`) et `Column::actions()`
    (collée à droite sous `md`) ; tokens `.th`/`.td` en `px-3 py-2` sous
    `md`, `th` sans retour à la ligne ; `ListLayout` / `FilterBar` resserrés
    (`p-4 pb-24`, titre `text-2xl`) — le `pb-24` libère le FAB (M10) ;
  - 17 pages taguées (dates, versions, détails → secondaires) ; dashboards :
    actions de ligne en icône seule sous `sm` ; orgs : rôle secondaire ;
  - `EditorShell` : barre sur 2 lignes sous `sm` (identité / actions
    alignées à droite), `#id` et chip version masqués, Historique et Debug
    en icône seule → Enregistrer / Déployer visibles sans glisser (M11).
- **2026-10-04 — lot 3 partiel** : `SearchInput { on_refresh }` colle le
  refresh au champ (un seul groupe flex) → plus de bouton seul sur une ligne
  (9 pages). Vérifié sur le téléphone : devices 196 → 61 px par ligne, plus
  aucune action hors écran sur dashboards / orgs / flows / devices.
- **2026-10-04 — suite** (retour user « le dashboard ouvert est moche ») :
  en-tête de la vue dashboard compacté (553 → 392 px : ← icône, titre
  tronqué, ✎ ; version + cadence en 2e ligne) ; « Ouvrir » garde son
  libellé dans la liste (œil, plus d'éclair « flash ») ; libellé
  « Rafraîchissement » masqué sous `sm` (accueil libéré, M14) ; recherche
  globale dans le drawer mobile, qui se ferme à l'ouverture d'un résultat
  (M16) ; carte : panneau POI en overlay fermé par défaut sous `lg` (M13) ;
  en-tête de l'app masqué tant qu'un éditeur est monté
  (`ui::EDITORS_OPEN`, compteur), éditeur en `100dvh` (M12).
- **2026-10-04 — dialogues, alignements, POI** : `Modal` jamais plus haut
  que l'écran (corps défilant), `MODAL_FOOTER` épingle la rangée d'actions
  (FormDialog + 10 dialogues) ; `DANGER_BTN` en `inline-flex` (la poubelle
  était plus basse que ses voisins) ; actions dashboards / contrôles en une
  rangée flex de hauteur fixe ; stepper device compact ; pinout à 560 px
  minimum + défilement ; aperçu d'objet attaché à un POI (dashboard…) en
  plein écran sous `md` — il recevait 9 px à gauche du drawer et son en-tête
  débordait sur celui du drawer (retour user « des trucs se superposent »).
- **2026-10-04 — M7 tranché (user : « il faut harmoniser »)** : le bouton
  rafraîchir a **une seule place**, `ListLayout { on_refresh }` = dernier
  bouton de l'en-tête, juste après « Nouveau… » (seul en haut à droite sans
  bouton de création). Retiré des barres de recherche, d'onglets et de
  filtres ; `SearchInput { on_refresh }` supprimé ; visualisation passée sur
  `ListLayout`. Vérifié sur les 21 pages : 1 bouton, dans l'en-tête, en
  dernier. Seule exception assumée : le contrôle de cadence auto (accueil,
  vue dashboard), lui aussi en haut à droite. Règle dans le README du socle.
- **2026-10-04 — M6 + M17** : tables hors socle compactées sur le même
  principe que `DataTable` (padding `.th`/`.td`, colonnes secondaires
  masquées sous `md`, repliées dans le détail dépliable ou sous le nom) :
  événements (sujet/flow/nœud), journal des notifications (source/flow),
  modèles (entrée/labels/seuil/date + actions collantes), fournisseurs LLM
  (modèle sous le nom, clé masquée, actions collantes), clés de l'agent
  edge, orgs de `/admin/status` (offre sous le nom), dernières mesures de
  l'accueil. `ACTIONS_TH_CLASS` / `ACTIONS_TD_CLASS` publiques pour les
  tables maison. Détail org : sections `p-4` sous `md`, supprimer en icône,
  membres tronqués ; en-tête « Ajouter un fournisseur » empilé sous `sm`.
  Les deux dialogues de `system.rs` défilent (`max-h-full overflow-y-auto`).
  Vérification téléphone à faire après réinstallation de l'APK.

## 4. Décision à prendre (avant le lot 2)

Le registre retient « toutes les listes = tables, zéro grille de cartes »
(revirement 2026-09-18, constat **desktop**). Sur mobile, l'alternative au
tri de colonnes du lot 2 est une **liste empilée** (une ligne = un bloc :
nom en titre, 2 attributs, actions en pied). Le plan ci-dessus reste en
table (priorités de colonnes + défilement) pour respecter la règle ; à
trancher si le rendu du lot 2 ne suffit pas.
