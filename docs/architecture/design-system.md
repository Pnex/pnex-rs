# Système de design de l'UI (refonte 2026-10)

Référence de l'identité visuelle et des primitives de l'interface. La 0.1.0
avait posé les écrans sans passe UX/UI : chaque page réinventait ses boutons,
ses tailles et ses couleurs (90 boutons primaires recopiés, 5 rayons, des
tailles 10/11/13 px, trois styles de libellés). Ce document fixe les règles,
le code les porte (`style/tailwind.css`, `components/ui.rs`) et une garde CI
(`src/design_guard.rs`) empêche la dérive.

Maquette de la proposition : canvas « PNeX — nouvelle identité UI » (privé,
lien dans la session du 2026-10-11).

## 1. Principe : un poste d'instrumentation pour les données

PNeX est une plateforme de données d'origine industrielle. L'interface suit
le principe ISA-101 des IHM haute performance : le fond est calme et neutre,
la couleur ne sert qu'à porter une information.

- **Cyan « signal »** (le point du logo) : ce qui est interactif, sélectionné
  ou vivant (liens, focus, sélection, donnée qui bouge).
- **Croix vermillon « repère »** (le X du logo) : marque l'endroit où l'on est
  dans la navigation. C'est la seule touche forte, elle ne sert à rien d'autre.
- **Encre** : l'action principale d'un écran (un seul bouton plein par écran).
- **États** normal / inactif / alerte / alarme : couleur doublée d'une forme
  (rond, losange, triangle) pour rester lisibles sans vision des couleurs.

## 2. Jetons (`style/tailwind.css`, bloc `@theme`)

| Jeton | Clair | Rôle |
|---|---|---|
| `ink` | `#16212b` | texte, bouton principal |
| `slate` | `#4c5966` | texte secondaire, aides |
| `line` / `line-strong` | `#d5dbe1` / `#c3cbd3` | bordures, séparateurs / champs |
| `ground` | `#eef1f4` | fond de l'application |
| `rail` | `#e4e9ee` | barre latérale |
| `surface` | `#ffffff` | surfaces de travail |
| `signal` / `signal-soft` | `#0e7c9c` / `#e3f2f6` | interactif, sélection |
| `mark` | `#e5482d` | repère de navigation |
| `ok` / `idle` / `warn` / `alarm` | `#2b7a4b` / `#8793a0` / `#c77700` / `#b42318` | états |

Les échelles `gray-*` et `blue-*` de Tailwind sont **réaccordées** sur ces
teintes (gris acier, cyan signal) : les ~3 500 classes existantes suivent
l'identité sans réécriture. Raccourci assumé : `blue-*` est le cyan signal
sous son ancien nom, à renommer `signal-*` quand aucune autre session n'a les
pages en vol.

**Typographie** : IBM Plex Sans (400/500/600) et IBM Plex Mono (400/500),
sous-ensemble latin embarqué en data URI dans `style/fonts.css` (web, desktop
et APK hors ligne identiques). Licence SIL OFL 1.1 (`style/fonts-OFL.txt`) :
exception à la règle MIT/BSD/Apache, propre aux polices (OFL = licence
standard des polices libres, permissive, non contaminante). Le mono est
réservé au code, aux clés et aux identifiants.

**Échelle de tailles** : 12 / 14 / 16 / 20 / 24 / 32 px (`text-xs` … `text-3xl`).
Aucune taille arbitraire (`text-[11px]`), aucun libellé en majuscules, aucun
espacement de lettres : la garde `design_guard` les refuse.

**Rayons** : 6 px pour les contrôles (`rounded-md`), 8–10 px pour les surfaces
et modales, pastilles uniquement pour les états.

## 3. Mode sombre

`style/dark.css` redéfinit les variables sous `:root[data-theme="dark"]` :
échelles chromatiques de Tailwind **miroitées** (50 ↔ 950, une teinte « -50 »
reste la plus discrète), gris acier de nuit, cyan éclairci, `white` devient la
surface sombre. Toutes les pages suivent sans modification.

La préférence du profil (Clair / Sombre / Automatique) est appliquée par
`state::theme` (attribut `data-theme` sur `<html>`, `auto` suit le système et
l'écoute), mémorisée dans `pnex.theme` pour éviter le flash au démarrage. La
variante `dark:` de Tailwind suit cet attribut (`@custom-variant`).

## 4. Primitives (`crates/pnex-frontend/src/components/ui.rs`)

| Primitive | Usage |
|---|---|
| `BTN_PRIMARY` / `_SM` | action principale (encre) ; variante compacte pour barres d'outils et lignes |
| `BTN_SECONDARY` / `_SM` | annuler, exporter, actions secondaires |
| `BTN_GHOST` | action discrète dans une ligne |
| `BTN_DANGER` | suppression avant confirmation |
| `BTN_DANGER_SOLID` | suppression confirmée (dans la modale de confirmation seulement) |
| `INPUT`, `FIELD_LABEL`, `FIELD_HELP`, `FIELD_ERROR` | champs de formulaire, libellé en casse de phrase |
| `PANEL` | surface plate à filet (pas d'ombre) |
| `TAB_BAR`, `TAB`, `TAB_ACTIVE` | onglets soulignés (filet cyan sur l'onglet ouvert), `role="tablist"` / `tab` + `aria-selected` ; un onglet est une vue, jamais une action |
| `LabelTag` | label D42 en étiquette d'actif « clé \| valeur » |
| `StateMark` | état ISA-101 (forme + couleur + texte) |

Les composants du socle CRUD (`ListLayout`, `ListStates`, `FormDialog`,
`Modal`, `ConfirmDialog`, filtres) consomment ces primitives : une page qui
les compose hérite de l'identité.

- **Modales** : titre 20 px, pied sur fond `gray-50`, bouton de validation
  nommé par l'action (« Créer », « Ajouter », « Enregistrer » en édition) ;
  sur téléphone, feuille qui monte du bas de l'écran.
- **Pages vides** : une seule forme (cadre pointillé, phrase de ce qui manque,
  détail de ce à quoi ça sert). Un message vide distingue « rien dans l'org »
  de « rien ne correspond aux filtres ».
- **Labels** : saisie et filtres proposent les noms et valeurs déjà utilisés
  dans l'org (`GET /api/v1/resources/labels/catalog`, `<datalist>` natif).
  Ce sont des étiquettes de travail, pas un modèle : sur une fiche, le type
  et les propriétés d'ontologie passent avant, les labels restent discrets
  et ne sont pas ajoutés aux pages qui n'en ont pas (`ontology.md`, « Labels
  et ontologie »).

## 5. Navigation par tâche

Les URL ne changent pas, seul le menu est regroupé :

- Accueil
- **Surveiller** : Dashboards, Graphes rapides, Carte, Sites, Événements
- **Connaissance** : Pages, Ontologie, Dossiers
- **Médias** : Bibliothèque, Caméras, Flux audio, Visites 3D, Annotations
- **Devices** : Devices et agents, Catalogue, Firmware custom, Référentiels
- **Automatisation** : Flows, Fonctions, Contrôles, Notifications, Modèles
  vision, Mélanges fluides
- **Réglages** (sous un filet) : Organisation, Secrets, Système, État
  plateforme (admin)
- Pied de barre : Assistant IA (remplace le bouton rond flottant qui masquait
  du contenu), sélecteur d'org, identité (ouvre le profil), rail, déconnexion.

Les entrées « bientôt » sans route sont retirées du menu.

## 6. Règles pour une nouvelle page

1. Composer le socle (`ListLayout`, `ListStates`, `DataTable`, `FormDialog`) ;
   jamais d'en-tête ni d'état vide écrits à la main.
2. Boutons et champs par les constantes de `ui.rs` ; ajouter seulement des
   classes de mise en page (`w-*`, `flex-1`, marges). Une largeur qui doit
   battre `w-full` d'`INPUT` prend le suffixe `!` (`w-32!`).
3. Couleurs par les jetons ; une couleur vive doit dire un état.
4. Vérifier la page en clair **et** en sombre, en desktop **et** à 390 px.
5. Texte secondaire en `text-gray-500` au plus clair (`gray-400` échoue au
   contraste dans les deux thèmes ; il reste pour les placeholders et les
   états désactivés).
6. Couleurs de canevas SVG (nœuds, grilles, câbles) en
   `var(--color-…, #hex)` plutôt qu'en hexadécimal brut : le mode sombre
   les inverse comme le reste. Une couleur choisie par l'utilisateur
   (fond, symbole) reste telle quelle.
7. Un seul bouton plein par écran : dans un éditeur, « Enregistrer » tant
   qu'il y a des modifications, puis l'action suivante (« Déployer »).

## 7. Reste à faire

- Renommer `blue-*` → `signal-*` (raccourci du §2).
- Éditeurs plein écran : flows, dashboards, Studio et annotations revus en
  clair et sombre (2026-10-11) ; l'éditeur de firmware et les inspecteurs
  de chaque type de nœud/widget restent à parcourir un à un.
- Onglets de l'Ontologie au-dessus du titre de la page, à replacer sous
  l'en-tête (le style est partagé depuis `TAB_*`).
- Captures de la documentation et du site à refaire.
