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

L'échelle `gray-*` de Tailwind est **réaccordée** sur le gris acier : les
classes existantes suivent l'identité sans réécriture. Le cyan signal a sa
propre échelle `signal-50` … `signal-950` (renommée depuis `blue-*` le
2026-10-11) ; `design_guard` refuse toute classe `blue-*`.

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
- **Étiquettes** : saisie et filtres proposent les noms et valeurs déjà
  utilisés dans l'org (`GET /api/v1/resources/labels/catalog`, `<datalist>`
  natif). Ce sont des étiquettes de travail, pas un modèle : sur une fiche,
  le type et les propriétés d'ontologie passent avant, les étiquettes
  restent discrètes ; elles existent sur tout objet listable (D218, §7).

### Mouvement (2026-10-12)

Un poste d'instrumentation ne bouge pas pour décorer : le mouvement répond
seulement à un geste de l'utilisateur, et seulement là où il ne l'utilise
pas cent fois par jour (philosophie d'Emil Kowalski, « You don't need
animations »).

- **Animé** : entrée des modales et de la confirmation (fondu + `scale(0.96)`
  250 ms, feuille du bas depuis le bord sous `sm`, 300 ms), tiroirs mobiles
  (glissement 300 ms, seulement là où ils recouvrent la page), toasts
  (entrée et sortie par le bord droit), appui des boutons pleine taille
  (`scale(0.97)`, 150 ms), panneaux droits ouverts par un bouton
  (`.pnex-drawer-in-right` : versions, debug, assistant, firmware), menus de
  barre d'outils (`.pnex-pop-in` 150 ms depuis le bouton, `-up` pour le
  sélecteur d'org), `+` qui tourne en ×, coche de flash réussi
  (`.pnex-success-in`, seul moment « plaisir »), morph de l'explorateur de
  dossier (re-layout et force ↔ chrono, 400 ms).
- **En cours** : battement d'opacité `pnex-beat` (points de l'assistant,
  `.pnex-live-dot` du build en cours), jamais le texte qui clignote ; spinner
  de liste `.pnex-spinner-delayed` invisible pendant 250 ms (pas de flash
  sur un chargement rapide).
- **Jamais animé** : recherche globale et raccourcis clavier, navigation entre
  pages, repli du rail (largeur = recalcul de mise en page), valeurs live,
  graphiques, lignes de tables, accordéons d'inspecteur, panneaux ouverts par
  une sélection (inspecteurs flow et dossier, fiche POI), onglets.
- **Ingrédients** : courbes `--ease-out-ui` et `--ease-drawer` du `@theme`,
  classes `.pnex-*-in` de `tailwind.css` (`@starting-style`, sans JS),
  `transform` et `opacity` seulement, jamais `transition-all`.
  `prefers-reduced-motion` garde les fondus et retire les déplacements.
- Les sorties ne sont pas animées (Dioxus démonte tout de suite), sauf le
  toast qui reste monté 200 ms (`.pnex-toast-leaving`).

## 5. Navigation par tâche

Les URL ne changent pas, seul le menu est regroupé :

- Accueil
- **Surveiller** : Dashboards, Graphes rapides, Carte, Sites, Événements
- **Connaissance** : Pages, Ontologie, Dossiers
- **Médias** : Bibliothèque, Caméras, Flux, Visites 3D, Annotations
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
8. **Toutes les listes CRUD se ressemblent (règle permanente, 2026-10-14).**
   Une page qui liste des entités = `ListLayout` + `FilterBar` +
   `ListStates` + `DataTable` + `ListPager` (requête `limit`/`offset` de
   `PAGE_SIZE`, `count` total renvoyé par l'API). Références :
   `pages/devices.rs`, `pages/ontology/mod.rs`, `pages/cases/mod.rs`.
   - **Jamais de tuiles ni de cartes** pour une liste, et jamais un mélange
     liste + tuiles entre pages. Une vignette (média, caméra) se met dans une
     colonne du tableau.
   - **Pagination toujours présente**, même quand la liste est courte
     aujourd'hui. Une liste chargée en entier (`api::client::request_all`,
     parce qu'un sélecteur ou un éditeur en a besoin, ou parce que l'API
     n'est pas paginée) est paginée côté client avec
     `crud::pager::page_slice` ; jamais un `limit` au-delà de 100 (le
     serveur tronque sans le dire, O64).
   - **Barre de filtres** : un `FilterBar` (dans le slot `filters` ou en
     tête du corps), dans cet ordre : sélecteurs propres à la page, puis
     `SearchInput` (« Rechercher par nom… »), puis `LabelFilterInput` ;
     les actions (importer, exporter) vont dans le slot `actions` de
     l'en-tête, jamais dans la barre. Garde : `design_guard`
     (`list_filter_bars_are_complete`).
   - **Onglets dans le slot `tabs`** de `ListLayout` (jamais dans
     `filters` ni dans le corps).
   - **Création par le bouton Ajouter + `FormDialog`**, jamais par un
     formulaire en ligne au-dessus du tableau.
   - **Le bouton « Ajouter » est toujours visible** pour qui peut écrire :
     jamais caché derrière un filtre ou une sélection préalable. Si plusieurs
     choses peuvent être créées (types d'objet de l'ontologie), le bouton
     ouvre d'abord un sélecteur « Que voulez-vous créer ? » ; si rien n'est
     créable, ce sélecteur dit quoi faire (installer un pack…).
   - Ce qui n'est pas une liste CRUD (réglages, fiche détail, éditeur,
     rendu d'un tableau de bord, calendrier) n'est pas concerné.
9. **Tout ce qui se lit dans le temps a le même lecteur (règle permanente,
   2026-10-14).** Audio, vidéo, enregistrement caméra, replay de flux IP,
   frise d'un dossier d'enquête : `components/transport.rs` —
   `PlayButton` (icônes lecture/pause), `Clock` (heure en mono + jour ou
   durée), `ScrubBar` (plages couvertes, repères en losange, progression et
   curseur `signal`, clic/glisser/flèches), `SpeedPicker` (`×N`), dans une
   barre `BAR`. Les fichiers audio/vidéo passent par `MediaPlayer`, jamais
   par les contrôles natifs du navigateur (`controls: true`). Références :
   `pages/cameras/player.rs`, `pages/cases/workspace/timeline.rs`.
   **Frise à couloirs** (2026-10-16) : tout ce qui a plusieurs pistes dans le
   temps (événements, détections, acteurs, images d'une vidéo, classes vues)
   passe par `components/timeline` (`Timeline` + `Lane` + `Item`, axe
   `TimeBase` : heure murale, temps du média ou rang) — une règle, des
   couloirs, un curseur commun ; jamais une liste « 00 s / 03 s » ni une frise
   faite main. Références : `pages/cases/workspace/timeline.rs`,
   `pages/lab/timeline.rs`.
10. **Tout se fait aussi au doigt (règle permanente, 2026-10-17).** L'app
    tourne sur tablette et téléphone : chaque geste a un équivalent tactile
    visible à l'écran.
    - **Jamais une action réservée au clavier.** Entrée, Échap, les
      raccourcis (1–9, flèches, Suppr) accélèrent, ils ne sont jamais le
      seul chemin : un champ qui valide par Entrée a son bouton à côté
      (référence : le champ « nouvelle classe » du Lab,
      `pages/lab/workspace.rs`, `ClassAdd`), une bulle qui se ferme par
      Échap a sa croix.
    - **Rien qui ne s'atteint qu'au survol** (`hover:`) : une action
      révélée au survol est visible en permanence sous `lg`, ou
      accessible par un appui.
    - **Cibles d'au moins 36 px** (`h-9`) pour ce qui se touche en
      travaillant (palette, outils d'un éditeur, navigation d'images).
    - Glisser (dessiner une boîte, déplacer un nœud) passe par les
      événements `pointer*`, jamais `mouse*` seuls.

## 7. Concepts et vocabulaire (D218–D224, 2026-10-15)

Le produit est riche ; l'interface doit rester lisible. Chaque concept a
**un seul nom** dans chaque langue, **un seul composant** et **une seule
place**. Les décisions ci-dessous viennent du tour d'horizon du
2026-10-15 (labels, annotations, lecteur, versions) ; elles priment sur
les règles plus anciennes qui les contredisent.

### Lexique (une notion = un mot)

| Notion | FR | EN | N'est pas |
|---|---|---|---|
| Classement libre d'un objet, pour filtrer et chercher (D42) | Étiquettes | Labels | ni une propriété d'ontologie, ni une annotation |
| Fait métier typé d'un objet | Propriété | Property | une étiquette |
| Relation typée entre deux objets | Lien | Link | une étiquette |
| Ce qui est posé sur un média visuel (D219) | Annotation | Annotation | une note de dossier, une détection |
| Unité versionnée et publiable d'annotations | Calque d'annotations | Annotation layer | — (plus « ensemble ») |
| Cadres produits par un modèle sur une vidéo | Détections | Detections | des annotations |
| Boîtes de vérité terrain d'un jeu d'entraînement | Étiquetage | Labeling | des annotations |
| Noms des sorties d'un modèle de vision | Classes | Classes | des étiquettes |
| Clés qui découpent une série temporelle | Dimensions | Dimensions | des étiquettes |
| Voix distinguées par la diarisation | Locuteurs | Speakers | des étiquettes |
| Texte libre d'un dossier d'enquête | Note | Note | une annotation |
| Discussion sur un passage d'une Page | Commentaire | Comment | une annotation |
| Lieu sur la carte | Site / repère | Site / place | une annotation |
| Site internet parcouru par des règles de navigation (D323) | Site web | Website | pas « Site » seul (= repère de la carte) |
| Suite d'étapes YAML qui parcourt un site web (D323) | Règle (de navigation) | (Navigation) rule | pas « recette » / « recipe » |
| Groupe de menu : tout ce qui alimente PNeX (appareils, agents navigateur, sites web) | Sources | Sources | pas « Appareils » pour le groupe |
| Source continue (pistes audio et/ou vidéo : caméra IP, direct, fichier) | Flux | Stream | une caméra-appareil |
| Caméra gérée par PNeX (matériel, firmware, réglages de capture) | Caméra | Camera | un flux |
| État sauvegardé d'un objet | Version | Version | — |
| Programme externe construit sur les données d'une org, par un compte de service (D433) | Application | Application | ni « intégration », ni « connecteur » (= fournisseur) |
| Vue d'un dossier : chaque acteur est un fil dans le temps (storyline) | Histoire | Story | ni l'historique des versions, ni la frise |
| Événement d'un dossier qui déplace de l'argent (montant + payeur + bénéficiaire) | Versement | Payment | ni « paiement », ni « transaction » |
| Vue d'un dossier : diagramme de Sankey des versements | Argent | Money | pas « Flux » (= streams) |
| Reçu par un intermédiaire sans versement sortant documenté | Non tracé | Untraced | pas « manquant » |
| Part d'un lien « possède » ; chaînes de parts | Part détenue / Détention | Share held / Ownership | — |
| Personne au-dessus du seuil de détention calculé | Bénéficiaire effectif (calculé) | Beneficial owner (computed) | jamais présenté comme un fait |
| Espace de travail de création : vidéo, récit (D225) | Production | Production | pas « projet » (= firmware, Vision Lab) |
| Regroupement ordonné d'une production : chapitre, accroche… (D229) | Partie | Part | — |
| Événement d'une production, à sa place dans le récit (D226) | Séquence | Sequence | pas « scène » (= scène 360° du Studio) |
| Être de fiction d'une production (D230) | Personnage | Character | jamais une personne réelle |
| Unité visuelle d'une séquence : cadrage, prompt, rendu (D232) | Cadrage | Shot | pas « plan » (= plan d'étage) |
| Médias attachés à un objet : images, 3D, voix (D231) | Galerie | Gallery | pas « assets » |
| Fichier GLB d'un objet, vu dans la visionneuse 3D (D231) | Modèle 3D | 3D model | pas « modèle » seul (= modèle de vision) |
| Angles des articulations d'un modèle 3D, enregistrés sous un nom | Pose | Pose | — |
| Os d'un modèle 3D qu'on fait tourner | Articulation | Joint | pas « os », pas « bone » |
| Média produit par un fournisseur de génération (D235) | Généré | Generated | — |
| Extension de navigateur appairée qui exécute des missions, une par ordinateur (D255) | Agent navigateur | Browser agent | pas « collecteur », pas « extension » seule (= le logiciel à installer) |
| Ce qu'un agent navigateur exécute : une page, un compte, un live (D256) | Mission | Mission | pas « règle », pas « tâche » |
| Ensemble nommé de permissions, attribuable (iam.md D287) | Rôle | Role | le rôle d'un acteur dans un dossier |
| Rôle donné à un membre ou à une équipe (D288) | Attribution | Assignment | pas « affectation », pas un partage |
| Membres de l'org regroupés pour leurs accès (D286) | Équipe | Team | pas « groupe » (groupe froid, de sociétés, de nœuds) |
| Unité d'autorisation `service:verbe` (D289) | Permission | Permission | une autorisation du navigateur ou du device |
| Écran « qui peut faire quoi » (iam.md §9) | Accès | Access | pas « Partager » (lien public d'un tour) |
| Service externe qui répond à une requête de recherche sur le web (web-presence.md D360) | Fournisseur de recherche web | Web search provider | pas « moteur de recherche » (le service tiers), pas « recherche » seule (= recherche globale D69) |
| Filiales et sites d'un groupe sans contact ni statut client (account-mapping.md D330) | Espace blanc | White space | pas « zone blanche » (couverture réseau), pas « trou » |
| Ce qui a bougé sur un compte suivi : annonce légale, nouvel établissement, page modifiée (account-mapping.md D335) | Signal commercial | Commercial signal | pas « alerte » (= notification envoyée), pas « signal » seul (= signaux d'un dossier) |
| Rencontre avec un compte, ses participants, son enregistrement (account-mapping.md, réunions) | Rendez-vous | Meeting | pas « réunion commerciale » ni « RDV » |
| Point de la méthode de vente de l'org, suivi rendez-vous par rendez-vous | Critère du playbook | Playbook criterion | pas « note » ni « score » d'une personne : on agrège par critère |
| Écriture proposée par une machine (import, flow, détecteur, modèle), en attente d'un humain (proposals.md D371) | Proposition ; la page « À valider » | Proposal ; « To validate » | pas « suggestion » ni « recommandation » (= lien `recommended`) |
| Affaire suivie avec un compte, de la découverte à la signature (crm.md D449) | Opportunité | Opportunity | pas « affaire », pas « deal » |
| Ce qui est fait ou à faire pour un compte : appel, mail, message… (crm.md D450) | Activité | Activity | pas « tâche » seule (= une nature d'activité) |
| Étape du plan de relance de l'org : jour, nature, consigne (crm.md D450) | Étape de relance ; plan de relance | Follow-up step ; follow-up plan | pas « séquence » (= production), pas « cadence » (= lecture d'un pin) |
| Point exigé d'une opportunité avant une étape (crm.md D449) | Critère de qualification | Qualification criterion | pas « critère du playbook » (= rendez-vous) |
| Retrait des données personnelles d'un objet, qui reste en coquille (crm.md D446) | Effacement | Erasure | pas « suppression » (= archivage) |
| Boîte relevée par PneX (IMAP), personnelle, partagée ou d'un compte de service (mail.md D453) | Boîte mail | Mailbox | pas « compte mail », pas « email account » |

Une chaîne d'interface qui emploie un mot pour une autre notion est un
défaut, au même titre qu'une chaîne en dur. **Garde bloquante** :
`crates/pnex-frontend/src/lexicon_guard.rs` scanne les deux `.ftl` et les
fiches de l'assistant ; un concept nouveau ajoute sa ligne au lexique
**et** sa règle à la garde (mot interdit, mot à employer, clés exemptées
avec leur raison).

**Évolution notée (non planifiée)** : une caméra-appareil apparaîtrait
aussi comme source dans « Flux » — une seule liste de tout ce qui se
regarde ou s'écoute ; « Caméras » ne garderait que la configuration du
matériel (à traiter avec D223).

### D218 — Étiquettes : un seul mécanisme, sur tout objet, dans la recherche

- Les étiquettes D42 s'appliquent à **tout objet listable** : appareils,
  médias, tableaux de bord, visites, flows, sites et dossiers de la carte,
  Pages, dossiers d'enquête, objets d'ontologie, fonctions, firmwares,
  contrôles, flux, caméras, modèles, canaux et modèles de notification,
  calques d'annotations. Révise `ontology.md` (« labels non étendus à tous
  les kinds ») : l'étiquette reste un classement de travail, les faits
  métier restent des propriétés et des liens.
- **Même composant partout** : `LabelsEditor` sur la fiche ou dans la
  barre de l'éditeur (`LabelsButton`), `LabelChips` dans une colonne des
  listes, `LabelFilterInput` dans la barre de filtres de **chaque** liste.
- **Recherche globale** : une saisie `#valeur` ou `#clé:valeur` cherche par
  étiquette sur tous les types (`POST /resources/search`) ; une saisie
  libre trouve aussi les objets dont une étiquette contient le texte
  (groupe « Étiquettes »).
- **Surface, pas profondeur** : une étiquette appartient à l'objet qui la
  porte et à rien d'autre — pas d'héritage par dossier (supprimé le
  2026-10-15), pas de lien avec l'ontologie, le rangement ou les liens.
  Les types qui ne sont pas de l'ontologie (fonctions, firmwares,
  contrôles, flux, modèles, canaux et modèles de notification, calques
  d'annotations) portent des étiquettes sans devenir des objets
  d'ontologie (`LABEL_ONLY_KINDS`) ; leur suppression purge les étiquettes
  en base (trigger `pnex_labels_purge`).
- Un seul mot : « Étiquettes » / « Labels », jamais « label » en français.

### D219 — Annotations : ce qui est posé sur un média visuel

- Une annotation est **toujours ancrée** sur une photo, un plan, un
  panorama 360, une visite ou un splat ; un calque sans média ni visite
  n'existe plus (refus à la création, purge des calques libres).
- Son contenu peut être un **repère** (appareil, pin, état), une **note**,
  une **lecture** (valeur, sparkline, jauge, indicateur) ou un **contrôle**
  d'org : un plan ou un 360 « vivant » est l'intérêt même de la
  fonctionnalité (D128, D147). Ce qui n'est pas posé sur un média n'est
  pas une annotation (détections, étiquetage, notes de dossier).
- **Entrée depuis l'objet** : bouton « Annoter » sur la fiche d'un média
  visuel, dans l'éditeur de visite et sur un splat ; il ouvre l'éditeur
  d'annotations sur cet objet (créé au besoin). La page Médias ›
  Annotations reste l'index des calques.
- Les calques d'annotations portent des étiquettes (D218).

### D220 — Un seul lecteur pour tout ce qui se lit dans le temps

Extension de la règle 9 (§6) :

- **Transcriptions et segments** : un segment s'écoute avec `MediaPlayer`
  quand son audio est conservé ; un clic sur un horodatage positionne le
  lecteur.
- **Plages** (programmes, postes) : barres sur `ScrubBar` avec le curseur
  « maintenant », plus de frise faite main.
- **Fenêtre de temps** : un seul `WindowPicker` (`components/transport.rs`)
  avec les mêmes préréglages partout (1 h, 6 h, 24 h, 7 j, 30 j,
  personnalisé) ; plus de `select` propre à chaque page ni de
  `datetime-local` nu.
- **Bornes d'un extrait** (imports du Vision Lab) : choisies sur la
  `ScrubBar` du média, pas tapées à l'aveugle.

### D221 — Versions : un composant, une frise, trois verbes

- Un composant `VersionHistory` pour tout objet versionné (flow, tableau
  de bord, fonction, firmware, visite, calque d'annotations, Page, média,
  type d'ontologie) : en tête une **frise** (`ScrubBar`, un repère par
  version, la version active marquée, le curseur sur la version affichée,
  lecture = défilement des versions), puis la liste paginée.
- **Trois verbes, partout** : « Ouvrir » (afficher ou charger dans
  l'éditeur), « Comparer » (avec la version active, quand un diff a du
  sens), « Rétablir » (devient la version active). Les actions de cycle
  de vie propres à un objet (« Déployer » un flow, « Publier » une visite
  ou un calque) restent dans la barre de l'éditeur, pas dans l'historique.
- Même place : tiroir de droite ouvert par le bouton « Versions » de
  `EditorShell` ; sur une fiche sans éditeur (média, type), une section
  de la fiche.

### D222 — Éditeurs : une seule coquille

Tout éditeur plein écran passe par `EditorShell` + `StatusChip` (barre :
retour, nom, étiquettes, état, versions, action principale) : flows,
tableaux de bord, visites et dossiers déjà ; à migrer : annotations,
fonctions, firmware, Pages, Studio, type d'ontologie, pinout.

### D223 — Où s'ouvre un objet

| Forme | Quand |
|---|---|
| **Fiche** (route propre, bouton retour, lien partageable) | consulter ou gérer un objet : appareil, média, site, objet, dossier, flux… |
| **Éditeur** (`EditorShell`) | construire un objet : flow, tableau de bord, visite, annotations, Page… |
| **Tiroir de droite** | aperçu rapide sans quitter un contexte spatial (carte, canevas) ; il mène à la fiche |
| **Modale** | créer, confirmer, choisir — jamais consulter |

Conséquences : enregistrements et segments d'un flux passent d'une modale
à un onglet de sa fiche ; un média et un appareil s'ouvrent sur leur
route plutôt qu'en remplacement de la liste ; le tiroir des sites prend
la classe de tiroir commune.

### D224 — Primitives sans copie

- Plus aucune constante locale de bouton ou de champ (`const BTN`,
  `const INPUT`…) : `ui::BTN_*`, `ui::INPUT`. Une variante manquante
  s'ajoute à `ui.rs`.
- **Pastilles** : un composant `Badge` (ton neutre, signal, succès, alerte,
  danger) au lieu des `span rounded-full` écrits à la main ; `StateMark`
  pour les états ISA-101.
- **Chargement** : une seule marque, la **gerbe** (`components/gerbe.rs`),
  miniature de la gerbe de faisceaux du login : faisceaux qui convergent,
  segments aux couleurs de la palette du login qui y coulent (SVG + CSS,
  immobile sous `prefers-reduced-motion`, `inherit` sur fond sombre ou
  dans un bouton plein). Différée de 250 ms au-delà de 32 px. Un compteur
  des requêtes en cours (`state::ui::IN_FLIGHT`) fait couler la vraie
  gerbe du login (`assets/tron-gerbe.js`, attribut `data-pnex-gerbe`)
  derrière le logo de la barre latérale : 7 faisceaux fins et rapides,
  couleur atténuée (`--gerbe-glow` 0,38 en clair, 0,65 en sombre) pour que
  le logo reste lisible, après 250 ms ; la petite gerbe en barre repliée.
  On voit toujours les données arriver. Garde : `design_guard`
  (`no_hand_made_spinner`) ;
  **vide** : `ListStates` ou `EmptyState`, jamais un cadre pointillé écrit
  à la main.
- **Onglets** : `TAB_BAR` partout (assistant et sélecteur de ressources
  compris).
- `design_guard` refuse les constantes locales et les classes de bouton
  écrites en ligne.

### Mouvement

Les règles de §4 « Mouvement » s'appliquent ; pour ce chantier : tiroirs
et modales en `ease-out` court (≤ 250 ms), aucune animation sur ce qui
répond au clavier ou se répète (filtres, onglets, pager), retour de
pression `scale(0.97)` sur les boutons, `prefers-reduced-motion` respecté.

## 8. Plan de mise en œuvre (2026-10-15)

Chaque phase se termine par une passe clair/sombre, desktop/390 px, et
l'e2e a11y strict.

1. **Vocabulaire et étiquettes (D218)** : lexique appliqué aux deux `.ftl`
   et aux fiches de l'assistant ; étiquettes sur tous les objets listables
   (registre `resources`, éditeur, colonne, filtre) ; étiquettes dans la
   recherche globale.
2. **Annotations recentrées (D219)** : bouton « Annoter » sur média,
   visite et splat ; refus et purge des calques libres ; « Détections »,
   « Étiquetage », « Calque » dans l'UI ; documentation alignée (les
   tableaux de bord n'affichent pas les annotations).
3. **Lecteur partout (D220) et versions (D221)** : `WindowPicker`,
   transcriptions et segments écoutables, plages sur `ScrubBar`, bornes du
   Vision Lab ; `VersionHistory` avec frise et trois verbes, porté sur les
   onze historiques.
4. **Coquille, formes d'ouverture et primitives (D222–D224)** : éditeurs
   sur `EditorShell`, fiches en routes, `Badge`/`Spinner`/`EmptyState`,
   constantes locales supprimées, garde `design_guard` étendue.

## 9. Reste à faire

- Revue sombre faite le 2026-10-11 : toutes les pages de liste, les pages
  de détail (device, média, POI, objet, dossier), les éditeurs plein écran
  (flows, dashboards, Studio, annotations, firmware, pages) et l'inspecteur
  de chaque type de nœud (31) et de widget (35). Reste à voir au cas par
  cas : visionneuses 360/splat et carte avec un fond configuré.
- Captures de la documentation et du site à refaire.
- Lecteur temporel étendu aux versions : décidé (D221, §7).
