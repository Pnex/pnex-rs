# PRD — Graphe de dossier rendu par Sigma.js

| | |
|---|---|
| Statut | **En cours — 2026-10-12** : GS-L1 et GS-L2 implémentés, non commités ; GS-L3 et GS-L4 partiels (§9). Relu contre le code le 2026-10-12 (§8) |
| Contexte | Le graphe L4 natif (`cases-workspace.md` K31, `workspace/graph.rs`) devient illisible au-delà d'une quarantaine de nœuds : rangées collées aux bords, libellés superposés, pile de libellés d'arêtes autour des hubs, nœuds tous identiques |
| Repose sur | K31 (graphe biparti dérivé, borné, stable), K39 (accessibilité, thèmes, téléphone), disposition enregistrée par dossier (`case_layouts`, migration 000013, `GET/PUT /cases/{id}/layout`), expansion à facettes (`POST /cases/{id}/expand`), estompage au curseur (`faded`, `relation_at`), pont JS de la carte (`map_viewer.rs` + `js/map.js`) |
| Remplace | La partie **rendu** de K31 (SVG natif). Le principe K31 reste : graphe dérivé, borné à 300 nœuds (`CAP`), stable d'une ouverture à l'autre |
| Numérotation | **GS1…GS13**, lots **GS-L1…GS-L4** |

## 1. Le besoin

Sur la capture de l'affaire de référence (34 événements, environ 75 nœuds), le graphe ne se lit pas. Les causes, trouvées dans le code :

| Symptôme | Cause | Où |
|---|---|---|
| Rangées parfaites en haut et en bas | Les nœuds peu ou pas reliés sont repoussés par `k²/d²`, l'attraction au centre (`* 0.06`) ne compense pas, et `clamp(x0, x1)` les colle au bord du cadre | `pnex-core/src/ontology/layout.rs` l. 119–125 |
| Pile « Acteur / about » au centre | Toute arête allumée (`lit`) affiche son libellé au milieu ; un hub en a 15 | `workspace/graph.rs`, rendu des arêtes |
| Texte illisible sur la rangée du bas | Au-delà de `LABELS_UNDER = 40`, les voisins du nœud ciblé affichent tous leur libellé à `y + 24`, sans halo ni collision | `workspace/graph.rs`, rendu des nœuds |
| Tout se ressemble | Un seul style d'objet (cercle cyan r = 9), `GNode` ne porte pas le `type_key` (seulement `id`, `title`, `event_from`, `extra`) | `workspace/graph.rs` l. 35 |

On veut un graphe qui se lit sans clic, qui isole le voisinage d'un nœud au survol, et une seconde lecture chronologique du même dossier.

## 2. Décisions

### GS1 — Sigma.js v3 + graphology, en module JS au pattern de la carte

- Nouveau module `crates/pnex-frontend/js/graph.js`, bundle esbuild IIFE **minifié** vers `assets/graph.js`, script `js:graph` dans `package.json` (au format de `js:doc`), ajouté à la tâche `task js:build` avec son stub (`test -f … || printf …`) comme pour `map.js`. Chargé dans `main.rs` comme `map.js`.
- Dépendances npm **épinglées en version exacte** (pas de `^`, contrairement à `maplibre-gl` et `gsplat` aujourd'hui) : `sigma@3.0.2`, `graphology@0.26.0`, `graphology-layout-forceatlas2@0.10.1`, `graphology-layout-noverlap@0.4.2`, `@sigma/edge-curve@3.1.0`, `@sigma/node-border@3.0.0`. Toutes MIT : lire le `LICENSE` réel de chacune à l'installation. Vérifier à ce moment que les `peerDependencies` des deux paquets `@sigma/*` acceptent `sigma@3.0.2`, sinon aligner les trois. Bundle mesuré sur le prototype : environ 230 Ko minifié.
- Global exposé : `window.pnexGraph`. Pont Rust dans un nouveau `src/graph_viewer.rs`, calqué sur `map_viewer.rs` (`wasm::call` sur wasm, `document::eval` en natif).

### GS2 — Contrat Rust ↔ JS

Appels (toutes les données en JSON, R13) :

| Fonction | Rôle |
|---|---|
| `mount(hostId, optsJson)` → bool | Crée le rendu dans le div hôte. `opts` : `caseId` (graine), `canWrite`, `canDrag` (faux sur téléphone, K39) |
| `setData(hostId, json)` | Remplace nœuds et arêtes (§GS3). Les nœuds déjà connus gardent leur position |
| `setState(hostId, json)` | `cursor`, `selected`, `fade` (par nœud), `shown` (par arête), `hiddenFamilies`, `mode` (`force` / `chrono`) |
| `relayout(hostId)` | *Réorganiser* : efface les positions et relance le layout |
| `fit`, `zoomIn`, `zoomOut` | Caméra |
| `positions(hostId)` → JSON | Positions courantes, pour l'enregistrement et les tests e2e |
| `screenPos(hostId, nodeId)` → JSON | `{x, y}` en pixels du viewport (`graphToViewport` + rectangle de l'hôte), pour les tests e2e |
| `unmount(hostId)`, `error(hostId)` | Comme la carte : `error` ∈ `lib`, `webgl`, `host` |

Événements vers Rust par polling, école `map_viewer.rs` (`take_click(prev_seq)`, pas de closure traversante) : `window.__pnexGraphEvent = {seq, kind, id}` avec `kind` ∈ `click`, `dblclick`, `stage`, `drop`. Pour `drop`, la position finale est lue via `positions`. Un seul slot : le dernier événement gagne, comme `__pnexMapLastClick` ; c'est suffisant pour des gestes humains.

### GS3 — Données transmises

- `GNode` gagne `type_key` (déjà présent dans `ObjectRef`, il suffit de le recopier dans `build()`) et `family` (§GS6). Le degré est calculé côté JS.
- Arête : `a`, `b`, `kind` (`role` / `relation`), `label` (rôle traduit par `role_label`, ou `link_type` sans `_`).
- `build()` reste en Rust, avec son plafond `CAP = 300` et son compteur de masqués. Ses tests restent valides.

### GS4 — Disposition par forces : ForceAtlas2 + noverlap

- **Repère** : celui de `layout.rs`, `[0, SIDE·√aspect] × [0, SIDE/√aspect]`, élargi aux nœuds déjà placés. Les positions enregistrées dans `case_layouts` restent dans ce repère : pas de migration, et l'aiguillage GS11 peut repasser au SVG sans perdre une disposition. Sigma cadre la caméra sur l'étendue du graphe, l'échelle absolue ne compte pas.
- Positions de départ : positions enregistrées sinon `hashed(caseId, key)` de `layout.rs` porté en JS **à l'identique** : FNV-1a 64 sur les octets UTF-8 de `caseId`, un octet `0x00`, puis les octets UTF-8 de `key` ; `x = (h >> 32) / 0xffffffff`, `y = (h & 0xffffffff) / 0xffffffff`, puis mis à l'échelle du repère. En JS : `TextEncoder` et `BigInt` (multiplication modulo 2⁶⁴ par `BigInt.asUintN(64, …)`), pas de `Number`, qui perd les bits de poids faible. **Vecteurs de test croisés** : un test Rust fige `hashed("case-1", "obj-1")` et deux autres couples, le test JS vérifie les mêmes valeurs.
- Nœuds placés ou épinglés : attribut `fixed` de graphology, ForceAtlas2 ne les déplace pas. Seuls les nouveaux venus bougent (comportement incrémental de K31 conservé).
- ForceAtlas2 synchrone, 600 itérations, `inferSettings` puis `gravity: 0.6`, `scalingRatio: 12`, `strongGravityMode: true`, `slowDown: 3`, `barnesHutOptimize: false` (sous 300 nœuds). Puis `noverlap` : `maxIterations: 300`, `margin: 10`, `ratio: 1.6`, `expansion: 1.1`. Budget : moins de 150 ms pour 300 nœuds sur le portable de référence, mesuré en GS-L1 ; au-delà, passer en worker (`graphology-layout-forceatlas2/worker`) plutôt que baisser les itérations.
- **Nœuds de degré 0 exclus de la simulation** et rangés en colonne à droite du dessin, centrée verticalement, triés par titre. Fini les rangées collées aux bords.
- `layout.rs` reste en place pour le repli (§GS11) et ses tests ; il n'est pas supprimé dans ce chantier.
- **Vue Hiérarchie (2026-10-18)** : bouton « Hiérarchie » (Sigma seulement), troisième `mode` de `setState` à côté de `force` et `chrono`. Les relations pointent vers le bas (propriétaire → possédé, financeur → financé, média → émission) ; `member_of` et `party_member` sont lus à l'envers (le groupe au-dessus de ses membres). Étage = plus long chemin depuis une racine, chaque cycle coupé là où un parcours depuis les racines le ferme ; étage trié par la position moyenne des parents, replié en sous-rangées au-delà de 24 ; les objets sans relation en rangées dessous. `hierarchyLayout` dans `graph_layout.js`, testé. Correctif au passage : un nœud sans lien, rangé dans la colonne de côté, n'est plus figé là ; il rejoint la simulation quand ses relations arrivent (avant, les relations chargées après le premier dessin laissaient tous les médias en colonne).
- **Grands dossiers (2026-10-18)** : Sigma dessine jusqu'à **2 000 nœuds** (`SIGMA_CAP`, un parlement tient dans un dossier) ; le repli SVG garde 300 (`CAP`). Au-delà de 300 nœuds : `barnesHutOptimize` et itérations ForceAtlas2 / noverlap réduites en `300/n` (planchers 120 et 40), environ 1,2 s à 2 000 nœuds, mesuré par `graph_layout.test.js` ; un statut « Disposition du graphe… » s'affiche avant (deux images, puis le calcul). Le serveur garde autant de positions (`layout.rs` `MAX_NODES` = 2 000).

### GS5 — Vue chronologique

- Bascule segmentée **Forces / Chronologie** dans la barre d'outils du graphe, mémorisée par utilisateur comme la bascule Graphe / Carte.
- Événements sur l'axe X dans l'ordre de `event_from`, avec des écarts compressés : `x += 26 × clamp(Δjours / 60, 1, 5)`. Les années creuses ne mangent pas l'écran (même intention que le mode *Compact* de la frise, sans ses coupures étiquetées).
- Trois rangées d'événements (`y` = 0, +55, −55) pour éviter les chevauchements, attribuées par balayage.
- Un couloir par famille d'objets (§GS6) : personnes et pièces au-dessus, organisations, lieux et identifiants en dessous, affirmations et hypothèses au-dessus après les pièces, `autre` le plus loin. Base du couloir `±(190 + 230 × (rang − 1))`, sous-rangées à 60 d'écart. Un objet se place à la moyenne des X de ses événements.
- Non reliés et non datés : colonne à gauche.
- Les libellés d'événements portent le mois (`2007-02 · Rossfield verse…`).
- La disposition chronologique est **dérivée, jamais enregistrée**. Un glisser y est permis mais n'écrit pas `case_layouts`.
- Transition animée entre les deux vues (`animateNodes` de `sigma/utils`, 700 ms ; 0 si `prefers-reduced-motion`).

### GS6 — Encodage visuel

- **Famille** : table fixe `type_key → famille` dans `graph.rs` (Q-GS1 tranchée, §7), sur les types qui existent aujourd'hui (pack `investigation.yaml` et types système) :

  | Famille | `type_key` | Couleur |
  |---|---|---|
  | `person` | `person` | `--color-signal` |
  | `organization` | `organization` | `--color-ok` |
  | `place` | `place` | `--color-warn` |
  | `piece` | `piece` | `--color-mark` |
  | `claim` | `claim`, `hypothesis` | `--color-mark` en anneau (§ ci-dessous) |
  | `identifier` | `identifier` | `--color-warn` en anneau |
  | `other` | tout le reste (types système, maintenance, types d'org) | `--color-idle` |

  Cinq tokens pour sept familles : `claim` et `identifier` réutilisent une couleur avec une forme différente, plutôt qu'introduire des tokens hors `design-system.md`. Les événements ne sont pas une famille, ils ont leur propre forme.
- **Événements en anneau** (programme `@sigma/node-border` : anneau 0,32 couleur `--color-ink`, cœur `--color-ground`). Objets pleins, liseré du fond. Les losanges actuels du SVG demandent un programme WebGL maison : hors périmètre (§3) ; la fiche de l'assistant, qui dit « events are diamonds », est corrigée dans le même lot (GS-L4).
- Nœud ajouté par une expansion (`extra`) : anneau double, à la place du pointillé actuel (`stroke_dasharray: "3 3"`).
- **Taille selon le degré** : objet `min(17, 5,5 + 2,4 √deg)`, événement `5 + 1,1 √deg`.
- **Arêtes** : rôle en trait droit, relation d'ontologie en courbe (`@sigma/edge-curve`) à la place du pointillé, que WebGL ne trace pas.
- Légende en surimpression, chaque famille filtrable (`hiddenFamilies`). Un nœud masqué par sa famille ne compte pas dans le badge « N masqués », réservé au plafond.

### GS7 — Libellés

- Grille de densité de Sigma : `labelDensity: 0.7`, `labelGridCellSize: 110`, `labelRenderedSizeThreshold: 7`. Elle remplace `LABELS_UNDER`, supprimée du rendu Sigma (elle reste dans `SvgGraph`).
- Dessin canvas maison (`defaultDrawNodeLabel` remplacé) : halo de 4 px couleur du fond, texte `--color-ink`, IBM Plex Sans 12 px, 600 pour le nœud ciblé. Titre tronqué à 30 caractères, titre complet dans l'encart de survol (`defaultDrawNodeHover` remplacé, même règle canvas).
- **Libellés d'arêtes** : seulement si le nœud ciblé a 6 liens ou moins, ou au survol de l'arête elle-même (`enableEdgeEvents: true`).

### GS8 — Focus et estompage

- Focus = nœud survolé, sinon nœud sélectionné. Hors voisinage direct : couleur mélangée au fond à 78 %, libellé masqué, arêtes à 12 % d'opacité. Voisinage : libellés forcés (`forceLabel`), arêtes allumées `--color-signal` épaisseur 2. Le tout par `nodeReducer` / `edgeReducer`, sans toucher aux attributs du graphe.
- **L'estompage au curseur reste calculé en Rust** par `faded()` et `relation_at()`, déjà testés, et transmis par `setState` (`fade`, `shown`). JS l'applique sans le recalculer. Relation fermée (`Gone`) : masquée ; à venir (`Ahead`) : estompée.

### GS9 — Interactions, à parité avec l'existant

- Clic : `ws.select_event(id)` ou `Sel::Object(id)`, comme aujourd'hui. Clic sur le fond : désélection. Échap : désélection.
- Double-clic sur un objet : offre d'expansion (`open_offer`), inchangée. Le zoom au double-clic de Sigma est coupé (`doubleClickZoomingDuration: 0` et `preventSigmaDefault` sur `doubleClickNode`).
- Glisser : le nœud est épinglé pour la session ; au lâcher, si `canWrite`, `PUT /cases/{id}/layout` (mêmes bornes, même R2). Un lecteur déplace sans enregistrer. Le cadrage ne bouge pas au lâcher (`autoRescale` coupé pendant et après le glisser, comme `held` aujourd'hui). **Téléphone : pas de glisser** (K39), `canDrag: false`.
- *Réorganiser* : efface la disposition enregistrée puis `relayout`.
- Zoom molette, panoramique, −, +, Ajuster : natifs de Sigma (`minCameraRatio: 0.15`, `maxCameraRatio: 3`, `stagePadding: 70`).
- Plafond de 300 nœuds et badge « N masqués » inchangés.
- **Accessibilité** : parité avec le SVG actuel, qui est un `role: "img"` étiqueté `cw-links` sans nœud focalisable. L'hôte garde ce rôle et cette étiquette ; sélection au clavier par la frise et l'inspecteur, comme aujourd'hui. Un graphe navigable au clavier est hors périmètre (§3).

### GS10 — Thème

- Couleurs lues avec `getComputedStyle` sur les variables `--color-*` au montage, puis à chaque changement de `data-theme` sur `<html>` (posé par `state/theme.rs`, y compris en mode auto) : un seul `MutationObserver` sur cet attribut suffit, pas besoin d'écouter aussi `prefers-color-scheme`. Aucun hex en dur hors valeur de repli.
- Respect de `design_guard` : pas de palette `blue-*`.

### GS11 — Repli sur le SVG natif

- Script absent (stub de build), WebGL indisponible ou `mount` en échec : le composant affiche le graphe SVG actuel, renommé `SvgGraph`, avec un badge discret « rendu simplifié ». Jamais de panic (école `media_viewer.rs`).
- `Graph` devient un aiguillage : hôte Sigma si `mount` répond `true`, sinon `SvgGraph`.
- Cibles concernées : le SVG reste le rendu effectif partout où WebGL manque. **WebKitGTK (bureau Linux) a WebGL souvent désactivé** : le harnais `task e2e:linux` doit être lancé pour vérifier que le repli s'affiche, pas Sigma.

### GS12 — Sécurité et dépendances

- **R11** : libellés et encarts dessinés sur canvas (`fillText`), jamais d'`innerHTML`. L'inspecteur reste en rsx. La légende en surimpression est rendue en rsx, pas par le module JS.
- **R13** : toutes les données vers JS en JSON (`JsValue` sur wasm, `serde_json::to_string` en natif).
- **R2** : l'écriture de la disposition reste le handler existant, inchangé.
- Aucune requête réseau depuis le module JS. Versions npm exactes, licences MIT consignées dans `docs/inventory.md`, `task security:deps` vert.

### GS13 — Assistant (D142–D145)

- Pas de nouvel outil ni de nouveau nœud : seule la fiche `assistant-kb/cases.md` change (section *Links*) : anneaux au lieu de losanges, couleurs par famille et légende filtrable, bascule Forces / Chronologie, « rendu simplifié » quand le navigateur n'a pas WebGL. Les losanges disparaissent dès GS-L1 : la fiche est corrigée dès GS-L1 pour les formes, puis complétée en GS-L3 (chronologie) et GS-L4 (légende).

## 3. Hors périmètre

Losanges pour les événements (programme WebGL maison), navigation clavier nœud par nœud, détection de communautés et enveloppes de groupes, mini-carte, regroupement d'arêtes, export PNG du graphe. À reprendre si le besoin se confirme.

## 4. Lots

| Lot | Contenu | Critère d'acceptation |
|---|---|---|
| **GS-L1** | `js/graph.js` (mount, setData, setState, layout forces GS4, encodage GS6, libellés GS7, focus GS8), `graph_viewer.rs`, aiguillage et repli GS11, fiche assistant (formes) | Le dossier de référence s'affiche sans rangée collée aux bords ni libellé superposé, en clair et en sombre ; WebGL coupé (`--disable-webgl`) → SVG actuel avec le badge ; layout de 300 nœuds sous le budget GS4 |
| **GS-L2** | Interactions GS9 : sélection, double-clic, glisser + enregistrement, Réorganiser, caméra | Le test e2e « a node moved in the graph keeps its place after a reload, until re-layout » passe, réécrit au §5 |
| **GS-L3** | Vue chronologique GS5, bascule mémorisée, transition, fiche assistant (chronologie) | Les événements se lisent de gauche à droite dans l'ordre des dates ; aucun objet hors de son couloir |
| **GS-L4** | Légende filtrable, thème vivant GS10, fiche assistant (légende) | Changer de thème recolore sans recharger ; la fiche décrit les deux vues |

## 5. Tests

- **Rust** : `build()` transmet `type_key` et `family` ; table de familles : chaque `type_key` du pack `investigation` a une famille autre que `other` (garde contre l'ajout d'un type au pack sans couleur) ; vecteurs FNV figés (§GS4) ; tests existants de `faded`, `relation_at`, plafond et `Fit` inchangés.
- **JS** (au format de `js/track_cut.test.js`) : mêmes vecteurs FNV que Rust ; même graphe et même graine → mêmes positions ; nœuds fixes immobiles ; nœuds de degré 0 dans la colonne ; vue chronologique : X croissant avec la date, chaque objet dans le couloir de sa famille.
- **e2e** : le canvas n'a plus de `data-testid="case-graph-node"`. Le test de glisser de `e2e/tests/cases.spec.ts` (l. 254) remplace `centre(text)` (boîte du `circle`) par `pnexGraph.screenPos(hostId, id)`, et `shape()` par un calcul sur `pnexGraph.positions(hostId)` ; le geste souris et les assertions sur `GET /cases/{id}/layout` ne changent pas. Un test de repli lance la page sans WebGL et vérifie le badge et `case-graph-node`.

## 6. Rappels pour l'implémentation (CLAUDE.md)

- Code et commentaires **en anglais**, y compris dans `graph.rs` dont des commentaires voisins sont en français.
- Nouvelles clés dans **les deux** `.ftl` : `cw-graph-mode-force`, `cw-graph-mode-chrono`, `cw-graph-simplified`, `cw-graph-family-<famille>` pour les sept familles de GS6.
- Builds via `scripts/guarded.sh` ou `task`, jamais deux compilations en parallèle. Le front se valide en `--target wasm32` (signals `mut`).
- `task fmt` avant commit, `task graph:update` après.
- Grille de sécurité `security.md` §6 avant commit, `/security-review` avant merge.
- Commits au format Conventional Commits, par exemple `feat(cases): sigma graph renderer (GS1-GS4)`.

## 7. Questions tranchées et ouvertes

- **Q-GS1 — tranchée** : table fixe `type_key → famille` dans le frontend, repli sur `other`. `ObjectTypeDef` (D190) porte un `icon` mais aucune catégorie ; en ajouter une toucherait le format des packs et de l'export pour sept lignes de table. À rouvrir si les types d'org deviennent nombreux dans les dossiers.
- **Q-GS2 — ouverte** : charger `graph.js` au démarrage comme `map.js`, ou à la première ouverture d'un dossier ? Par défaut comme `map.js`, par cohérence ; à revoir si les 230 Ko pèsent au démarrage mobile.
- **Q-GS3 — tranchée** : pas de disposition chronologique enregistrée (dérivée, GS5).

## 8. Relecture contre le code (2026-10-12)

Ce qui a changé par rapport au premier jet, et pourquoi :

| Point | Avant | Après | Raison |
|---|---|---|---|
| Familles | personne, organisation, flux et comptes, document et procédure | person, organization, place, piece, claim, identifier, other | Aucun type « flux », « compte » ni « procédure » n'existe ; `place` manquait alors que c'est le type que montre la carte |
| Port FNV | « même fonction » | octets exacts, `BigInt`, vecteurs croisés | Le `\0` entre graine et clé et la division par `u32::MAX` ne se devinent pas ; `Number` perd des bits |
| Repère des positions | non dit | celui de `layout.rs` | Sinon les dispositions enregistrées et le repli SVG divergent |
| Téléphone | glisser partout | `canDrag: false` | K39 : pas de glisser sur téléphone |
| Thème | `MutationObserver` + `matchMedia` | `MutationObserver` seul | `state/theme.rs` pose déjà `data-theme` en mode auto |
| Double-clic | non dit | zoom Sigma coupé | Sinon chaque offre d'expansion zoome aussi |
| e2e | « helper caméra » | `screenPos` dans le contrat, test existant nommé | Le seul test graphe (glisser + recharger + réorganiser) lit des `circle` |
| Bureau Linux | non dit | repli attendu | WebKitGTK sans WebGL |
| Assistant | fiche en GS-L4 | dès GS-L1 | La fiche dit « events are diamonds », faux dès GS-L1 |

## 9. État de l'implémentation (2026-10-12)

Fait, testé :

- **GS1–GS4, GS6–GS9, GS11, GS13** : `js/graph.js` + `js/graph_layout.js` (pur, testé par `bun test js/`), pont `src/graph_viewer.rs`, aiguillage dans `workspace/graph.rs` (Sigma si `mount` répond, SVG sinon avec le badge `cw-graph-simplified`). Bundle 210 Ko minifié, câblé dans `package.json`, Taskfile, Dockerfile et CI.
- Vecteurs FNV croisés Rust (`hashed_matches_the_js_port`) et JS.
- e2e : glisser + recharger + Réorganiser réécrit sur `pnexGraph` ; nouveaux tests du repli sans WebGL et de l'expansion au double-clic. Suites `@cases` et `@ontology` vertes (27/27) sur build local.
- Fiche `assistant-kb/cases.md` à jour (formes, couleurs, focus, repli).

Écarts au texte ci-dessus, mesurés :

- **Versions** : `sigma@4.0.0` sortie le 2026-10-08, trop récente ; on reste en 3.0.2. `graphology` passe en 0.26.0 (janvier 2025).
- **Budget GS4 non tenu au plafond** : ~40 ms à 75 nœuds, ~300 ms à 300 nœuds (synchrone, à l'ouverture, à *Réorganiser* et à l'arrivée d'un nouveau nœud). Noté en `shortcut:` dans le code ; worker à itérations fixes si les gros dossiers gênent.
- **Libellés GS7** : la grille de Sigma seule laissait des chevauchements. Réglage retenu : `labelDensity: 3`, `labelGridCellSize: 90`, `labelRenderedSizeThreshold: 4`, plus un test de collision au dessin (seul le nœud ciblé passe outre).
- **Calcul sur graphe détaché** : Sigma refuse un nœud sans `x`/`y` dès l'ajout ; la disposition se calcule sur une copie, importée ensuite.
- **Téléphone** : le glisser n'écoute que la souris ; au toucher on panoramique, ce qui tient lieu de `canDrag: false` (aucune détection téléphone dans l'espace de travail).

Reste à faire :

- **GS-L3** : `chronoLayout` et la transition existent et sont testés ; manquent la bascule Forces / Chronologie dans la barre (clés `cw-graph-mode-*`) et sa mémorisation. `setState.mode` est toujours `force`.
- **GS-L4** : le thème vivant est fait (`MutationObserver` sur `data-theme`) ; manque la légende filtrable (`hiddenFamilies` est prêt côté JS, clés `cw-graph-family-*`).
- **Défaut connu** : quand il y a peu de nœuds reliés, la colonne des isolés se cale au bord droit et son libellé peut être coupé (`stagePadding` ne compte pas les libellés).
- Licences npm à consigner dans `docs/inventory.md` ; vérification visuelle sur le vrai dossier de référence et en Android/desktop.

