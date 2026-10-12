# Fiche benchmark — OpenAleph (+ FollowTheMoney, react-ftm)

Chemins relatifs : `OA/` = `bench/openaleph` (commit `82fed2e`),
`FTM/` = `bench/followthemoney` (clone `--depth 1` du tag
`v4.11.0`, commit `8aaaece`, version épinglée par `OA/requirements.txt:53`).
`RF/` = `OA/ui/src/react-ftm` (copie vendue de react-ftm dans l'UI).

---

## 1. Identité

| Élément | Valeur | Preuve |
|---|---|---|
| Dépôt | github.com/openaleph/openaleph (fork communautaire d'Aleph/OCCRP, maintenu par DARC) | `OA/mkdocs.yml:3` |
| Commit étudié | `82fed2e` « Merge pull request #134 … feat/rm-email-suggestions » | `git log -1` |
| Licence OpenAleph | **MIT**, « Copyright (c) 2014-2015 Friedrich Lindenberg / 2016-2020 Journalism Development Network, Inc. » | `OA/LICENSE.txt:1-4`, `OA/pyproject.toml:11` (`license = "MIT"`) |
| En-têtes de fichiers | Aucun en-tête de copyright/SPDX/GPL dans `aleph/` ni `ui/src/` (grep `SPDX\|Copyright\|GPL` : 0 résultat) | grep |
| Sous-dossiers sous autre licence | Aucun `LICENSE` secondaire dans le dépôt (`find -iname 'LICENSE*'` → seul `LICENSE.txt`). Pas d'édition entreprise. | find |
| FollowTheMoney (Python) | **MIT**, « Copyright (c) 2017-2024 Journalism Development Network, Inc. » ; dépôt github.com/opensanctions/followthemoney | `FTM/LICENSE:1-3`, `FTM/pyproject.toml:10,19` |
| FtM JS (`@alephdata/followthemoney` côté UI, aujourd'hui `@opensanctions/followthemoney`) | MIT | `FTM/js/package.json:2,21`, `OA/ui/package.json:6` |
| react-ftm (diagrammes réseau, tableur d'entités) | MIT (`package.json` amont) ; dépôt alephdata/react-ftm **archivé le 29/11/2022**, déprécié ; OpenAleph en embarque une copie dans `ui/src/react-ftm/` (157 fichiers) | https://raw.githubusercontent.com/alephdata/react-ftm/master/package.json (l.17 `"license": "MIT"`), https://github.com/alephdata/react-ftm |
| Libs de graphe front | `d3-force` ^3 (ISC), `dagre` ^0.8.5 (MIT), `bezier-js` ^6 (MIT), `react-draggable` (MIT) — rendu SVG maison, **pas de Cytoscape** | `OA/ui/package.json:21-24,39` |
| Dépendance dates | `prefixdate` (même auteur, MIT) pour les dates partielles | `FTM/followthemoney/types/date.py:5` |

**Ce que ça permet à PNEX (MIT)** : reprise de code autorisée (OpenAleph, FtM, react-ftm sont tous
MIT/ISC) à condition de conserver les notices de copyright dans les fichiers repris (ex.
`THIRD_PARTY_NOTICES`). En pratique PNEX est en Rust/Dioxus : on reprend des **algorithmes et
schémas** (dates à précision par préfixe, `temporalExtent`, `edge source/target`, expansion en deux
temps, origine par mapping), pas de code React. Les fichiers YAML de schéma FtM peuvent être repris
tels quels comme référence (MIT) avec attribution.

---

## 2. Ce que fait le projet

OpenAleph indexe documents et données structurées (fuite, registres, CSV) en entités FtM dans
Elasticsearch/OpenSearch + Postgres, avec recherche plein texte, cross-référencement et des
« Investigations » (collections `casefile`) privées où l'on bâtit des **listes, diagrammes
réseau et frises** (EntitySets) et où l'on **mappe des tableurs en entités**.
**Ce qui nous concerne** : (1) le modèle temporel FtM (dates à précision partielle, intervalles,
`temporalExtent`), (2) la séparation entité / « edge schema » / rôle, (3) la frise (liste + Gantt),
(4) le diagramme réseau avec expansion de nœud à compteurs et undo, (5) le mapping CSV → entités
avec provenance par origine, rechargeable.

---

## 3. Modèle de données pertinent

### 3.1 FollowTheMoney : entités, temps, arêtes

- **Interval** (abstrait) : tout objet borné dans le temps.
  `FTM/followthemoney/schema/Interval.yaml:1-22` — commentaire explicite « **Intervals are not graph
  edges** … there are intervals that link to only one thing (e.g. Sanction) or to much more than two »
  (l.2-11). `temporalExtent: start: [startDate, date], end: [endDate]` (l.17-22).
  Propriétés : `startDate`, `endDate`, `date` (« Date … that isn't explicitly its startDate or endDate »,
  l.24-35), `summary`, `description`, `recordId`, `sourceUrl`, **`proof` → Document** (reverse
  `provenIntervals`, l.50-56), `publisher`, `modifiedAt`, `retrievedAt` (l.80-88).
- **Event** : `extends: [Interval, Analyzable, Thing]` (`Event.yaml:4-7`), `required: name`,
  `featured: name, summary, date, location` (l.16-20). Propriétés propres : `location` (type
  address, texte libre, l.22-24), `important` (l.25-26), **`organizer` → LegalEntity** (l.27-33),
  **`involved` → LegalEntity** (reverse `eventsInvolved`, l.34-40). → Les « rôles » d'un événement
  sont **figés dans le schéma** (organisateur / impliqué), pas choisis à l'attache.
- **Interest** (abstrait, `extends Interval`) : `role` et `status` en texte libre (`Interest.yaml:7-11`).
  C'est l'endroit où FtM porte un rôle *qualifié* sur une relation.
- **Ownership** : `edge: source: owner, target: asset, label: "owns", directed: true, caption: [percentage]`
  (`Ownership.yaml:17-23`) ; `owner` → LegalEntity, `asset` → Asset (l.25-38) ; `percentage`,
  `sharesCount`, `ownershipType` (exemples beneficial/direct/indirect/ultimate, l.39-57).
- **Membership** : `edge: member → organization, label "belongs to", caption [role]`
  (`Membership.yaml:18-24`).
- **Thing** (racine de tout ce qui s'affiche) : `name`, `alias`, `country`, `notes`, `address`,
  `addressEntity`, **`proof` → Document** (reverse `proven`, `Thing.yaml:111-117`), `createdAt`,
  `modifiedAt`, `retrievedAt` (l.136-150, « not to be confused with an internal 'last updated' »).
- **Person** : `temporalExtent: start birthDate, end deathDate` (`Person.yaml:24-28`) ;
  **LegalEntity** : `incorporationDate` / `dissolutionDate` (`LegalEntity.yaml:27-31`) ;
  **Document** : `start: [date, authoredAt, publishedAt]`, pas de fin (`Document.yaml:19-23`).
- **Mécanique `temporalExtent`** : parsée dans `FTM/followthemoney/schema.py:174-176`, héritée du
  parent si absente, avec erreur « Conflicting temporal start properties » en cas d'héritage
  multiple divergent (`schema.py:222-237`). Sur une entité : `temporal_start` = **plus petite** valeur
  parmi les propriétés de début, `temporal_end` = **plus grande** parmi celles de fin
  (`FTM/followthemoney/proxy.py:356-379`) — tri lexicographique de chaînes ISO (valide grâce au
  format préfixe).
- **Edge schemata** : `edge_source`, `edge_target`, `edge_label` (défaut = label du schéma),
  `edge_directed` (défaut `True`) lus dans `schema.py:159-170` ; `self.edge = source and target`
  (l.164) ; validation « Missing edge source/target » (l.259-263). `featured` = propriétés montrées en
  priorité, chacune doit exister (l.242-244).
- **Graphe** : `Graph.add()` transforme une entité à schéma `edge` en une arête par paire
  (source, target) via `edgepairs()`, sinon en nœud + arêtes « valeur » vers les propriétés de types
  pivots (`FTM/followthemoney/graph.py:232-270`). Les nœuds peuvent être des **valeurs** (email,
  téléphone, adresse…) et pas seulement des entités (`graph.py:222-230`).

### 3.2 Temps : instant / période / incertitude

- **Type `date`** : ISO 8601 **par préfixe** : `2021`, `2021-02`, `2021-02-16`, `2021-02-16T21`,
  `…T21:48`, `…T21:48:52` sont valides, la précision est implicite dans la longueur
  (`FTM/followthemoney/types/date.py:15-22`). UTC obligatoire, **pas de semaines ni de plages**
  (« no support for calendar weeks (2021-W7) and date ranges (2021-2024) », l.21-22).
- Comparaison = longueur du **préfixe commun** amortie (`compare`, l.66-68) ; conversion
  `to_datetime` = **borne basse** (`2021-02` → `2021-02-01T00:00:00Z`, l.70-81).
- Sentinelles : `HISTORIC = "1001-01-01"`, `RELEVANCE_MIN/MAX` 1900/2100 (l.31-39).
- Instant = Event avec seulement `date` ; période = `startDate`+`endDate` ; incertitude = précision
  du préfixe (aucun champ « ± » ni « circa »).

### 3.3 Provenance par statement et versionnage

- **Statement** = un fait atomique `(entity_id, prop, value)` dans un `dataset`, avec
  `canonical_id` (après dédoublonnage), `lang`, `original_value` (valeur brute avant nettoyage,
  supprimée si identique), `first_seen`, `last_seen` (défaut = first_seen), `external` (fait observé
  dans une source externe, pas encore validé), `origin` (`FTM/followthemoney/statement/statement.py:21-34,67-106`).
- Identité d'un statement = `sha1("{dataset}.{entity_id}.{prop}.{value}[@lang]")` + drapeau externe
  (`statement.py:268-289`). Donc **une même valeur vue deux fois = un seul statement** dont on
  étend `last_seen` : c'est l'historisation « vu de… à… » sans table d'historique.
- Dans Aleph : entité éditée = `Entity` Postgres (`id`, `schema`, `data` JSONB, `role_id`,
  `collection_id`, `created_at/updated_at`) (`OA/aleph/model/entity.py:18-37`) ; `update()` réécrit
  tout `data` (`entity.py:43-57`) — pas de versionnage des entités éditées à la main, seulement
  `updated_at`. Les sommes de contrôle ne sont pas écrasables par l'utilisateur (l.50-54, sécurité).
  Les IDs sont **signés par collection** (`collection.ns.sign`, `entity.py:77`).
- Provenance des entités issues d'un tableur : `origin = "mapping:<id>"` et `proof = table_id`
  ajoutés à chaque entité (`OA/aleph/logic/mapping.py:41-42,59-60`).

### 3.4 Aleph : investigation, entity sets, timelines, diagrammes, mapping

- **Collection** : `CATEGORIES` dont `"casefile": "Investigations"` (`OA/aleph/model/collection.py:38-59`) ;
  propriété `casefile` : « cases do not serve as source material, but as a mechanism of analysis »
  (l.204-208) ; une collection externe ne peut pas être casefile (l.149-154). Le nom UI est
  « Investigation », le code dit « casefile ».
- **EntitySet** (`OA/aleph/model/entityset.py:36-60`) : `id`, `label`, `type` ∈ {`list`, `diagram`,
  `timeline`, `profile`} (l.39-45), `summary`, **`layout` JSONB** (l.51), `role_id`, `collection_id`,
  `parent_id`. Les membres = `EntitySetItem` à `judgement == POSITIVE` non supprimés (l.62-68).
- **EntitySetItem** (`entityset.py:284-300` env.) : `entityset_id`, `entity_id`, `collection_id`,
  `compared_to_entity_id`, `added_by_id`, `judgement` (enum `positive/negative/unsure/no_judgement`,
  l.19-30, avec algèbre `__add__`). Hérité des « profils » (fusion de doublons) : retirer un item
  d'une timeline = poser `judgement: no_judgement` (commentaire explicite
  `OA/ui/src/screens/TimelineScreen/TimelineScreen.jsx:124-136`).
- **Layout d'une timeline** = `{vertices: [{entityId, color}]}` (`OA/ui/src/components/Timeline/types.ts:12-19`) ;
  le front ajoute `edges: []` à l'enregistrement (`TimelineScreen.jsx:140-150`) : même format que le
  diagramme. Les éléments d'une frise sont **des entités FtM normales** (Event, Ownership, Person…)
  créées dans la collection puis attachées (`entitySetAddEntity`, `TimelineScreen.jsx:110-122`).
- **Layout d'un diagramme** = `{vertices, edges, groupings, settings}` sérialisé par
  `GraphLayout.toJSON()` (`RF/components/NetworkDiagram/layout/GraphLayout.ts:513-523`) ; Vertex =
  `id, type, label, fixed, hidden, color, radius, position, entityId` (`…/layout/Vertex.ts:12-20`),
  positions entières sur grille (`snapPosition`, l.85-87) ; Edge = `type, entityId?, propertyQName?,
  directed` (`…/layout/Edge.ts:9-16`).
- **Mapping** (`OA/aleph/model/mapping.py:16-43`) : `query` JSONB (la config FtM), `table_id`
  (l'entité Table source), `entityset_id` optionnel (ajouter le résultat à une liste/diagramme),
  `disabled`, `last_run_status`, `last_run_err_msg`.

---

## 4. Patterns UX observés

### 4.1 Frise (Timeline)

- **Deux rendus commutables** « List » / « Gantt chart » (boutons `list` / `gantt-chart`,
  `OA/ui/src/components/Timeline/TimelineActions.tsx:43-62`) ; rendu par défaut `list`, zoom par
  défaut `months` (`state.ts:168-177`).
- **Liste** : une ligne `<tr>` par entité : date de début + **libellé de la propriété source**
  (« Start date », « Date of birth »…), date de fin si présente, légende, bouton corbeille « Remove
  from timeline » ; date invalide → bandeau « Invalid or no date » avec icône d'alerte
  (`TimelineList/TimelineListItem.tsx:40-127`). Tri par début, les éléments **sans date en tête**
  (`state.ts:200-221`).
- **Gantt** : **une ligne par élément** (pas par acteur), barre positionnée en CSS par
  `--timeline-chart-item-start-day` / `end-day` = jours depuis le début de la fenêtre
  (`TimelineChart/TimelineChartItem.tsx:71-81`) ; largeur de jour 20 px / 5 px / 1 px selon
  zoom jours / mois / années (`TimelineChart/TimelineChart.scss:19-32`) ; classes `singleDay` /
  `multiDay` (l.89-94). Les éléments sans date sont **exclus** du Gantt (`TimelineChart.tsx:23-25`).
  Survol = popover (légende, début, fin) masqué dès qu'un élément est sélectionné (l.74-79, 93-97).
  Sélection = les autres éléments passent en `--muted` (l.88) ; clic sur le fond = désélection (l.69).
  Au premier rendu, le premier élément est centré par `scrollIntoView` (l.33-38).
- **Zoom** : 3 niveaux discrets jours/mois/années ; niveau « jours » désactivé si l'étendue ≥ 20 ans,
  « mois » si ≥ 100 ans, avec tooltip explicatif (`state.ts:26-27,231-263`,
  `TimelineZoomLevelSwitch.tsx:21-34`). Marges automatiques autour des données (1–3 mois ou 1–3 ans,
  `TimelineChart/util.ts:22-53`). Pas de zoom continu, pas de curseur, pas de glisser.
- **Clavier** : ↑/↓ déplacent le focus d'un élément à l'autre, Échap désélectionne, Espace/Entrée
  sélectionne (`util.ts:269-320`). Un élément nouvellement sélectionné reçoit le focus et défile
  en vue s'il est masqué par la barre latérale (`TimelineChartItem.tsx:46-65`).
- **Inspecteur latéral** : sélection → panneau `EntityViewer2` éditable (propriétés + couleur) ;
  chaque modification met à jour l'entité **et** le layout (`Timeline.tsx:103-121`).
- **Ajout d'élément** : bouton « Add item » → dialogue ; type par défaut **Event**
  (`TimelineItemCreateForm.tsx:296`) ; le sélecteur de type ne propose que les schémas ayant
  une propriété de début temporelle (`optionsFilter`, l.86-101). Formulaire adaptatif :
  champs de dates déduits de `temporalExtent` (masque `date` si `startDate` existe, masque
  incorporation/dissolution pour Person — « rather confusing UX », l.172-238), saisie libre
  `YYYY-MM-DD` contrôlée par regex (l.212-232) ; si le schéma est un edge (ex. Ownership),
  deux champs entité source/cible **avec autocomplétion depuis l'index** (l.240-267) ;
  sinon un champ légende requis (l.269-285).
- **Autocomplétion depuis l'index** : `fetchEntitySuggestions(schema, prefix)` interroge
  l'index sur la collection, filtré par schéma (`TimelineScreen.jsx:68-80`), throttle 200 ms,
  annulation au démontage, requête vide au montage (`util.ts:352-387`).
- **Ajout depuis ailleurs** : page entité / tableau d'entités → tiroir `EntitySetSelector` avec
  sections Lists / Diagrams / Timelines (+ création à la volée) (`OA/ui/src/components/EntitySet/EntitySetSelector.jsx:192-210`) ;
  la section Timelines n'apparaît que si l'entité est datable (`EntityScreen.jsx:127-132`,
  `EntityTable.jsx:270-276`). Ajout en lot depuis une sélection de tableau.

### 4.2 Diagramme réseau (react-ftm NetworkDiagram)

- **Moteur** : rendu **SVG maison** (`<svg>` + `<g id="zoom">`, `RF/components/NetworkDiagram/renderer/Canvas.tsx:297-311`),
  pan par `react-draggable` (`DraggableCore`, l.4,304), zoom molette ×1.5 vers le point visé
  (l.181-187), boutons ×3 (l.195-199). Mise en page **d3-force** (300 ticks synchrones) et
  **dagre** pour « arrangement en arbre ». Pas de Cytoscape, pas de WebGL.
- **Ajout depuis l'index** : capture `docs/assets/images/add_to_dia.png` (« add entity to network
  diagram dialog ») ; création d'entités/liens directement dans le diagramme (versées dans
  l'investigation) — `OA/docs/user-guide/103/network-diagrams.md` étapes 4 et 7.
- **Expansion de nœud** (capture `docs/assets/images/expand_dia.png`, vue) : double-clic sur une
  entité → menu contextuel « Add link » puis « Discover all links (11) », « Discover assets and
  shares (9) », « Discover source document (2) » : **compteurs par propriété inverse**.
  Implémentation : `onDoubleClick` → `showVertexMenu` si `hasExpand`, sinon mode dessin d'arête
  (`renderer/VertexRenderer.tsx:113-124`) ; `showVertexMenu` ouvre le menu immédiatement puis appelle
  `expandEntity(id, undefined, 0)` (**limit 0 = compteurs seulement**) et injecte les résultats
  (`NetworkDiagram.tsx:305-334`) ; choix d'une ligne → `expandVertex(vertex, [prop])` charge les
  entités, les ajoute à l'EntityManager, relance `layout()` centré sur le viewport, **sélectionne
  les nouvelles entités**, toast « N vertices, M edges added » ou avertissement « rien de nouveau »,
  puis pousse dans l'historique (`NetworkDiagram.tsx:340-385`). Bouton toolbar « Expand »
  actif seulement avec exactement 1 sommet sélectionné (`toolbox/Toolbar.tsx:202-203`).
  API : `GET /api/2/entities/<id>/expand?…&limit=` (`OA/ui/src/queries.js:255-262`).
- **Undo/redo (rollback)** : pile `History` de `{layout, entityChanges}` ; un `push` tronque le futur ;
  `go(±1)` renvoie le layout cible + les mutations d'entités à rejouer/annuler
  (`RF/components/NetworkDiagram/History.ts:9-66`). Une expansion est donc annulable d'un Ctrl-Z
  (layout + entités créées).
- **Suppression** : supprimer un sommet-entité supprime aussi les entités-arêtes qui y sont liées ;
  un sommet-valeur (email, pays…) est seulement **masqué** (`hidden = true`)
  (`GraphLayout.ts:418-450`).
- **Groupements** : zones nommées ; un sommet déposé dans le rectangle d'un groupe y est ajouté
  (`GraphLayout.ts:452-483`) ; liens d'attraction faibles entre membres dans le force layout.
- **Persistance** : chaque modification propagée → `updateEntitySet(id, {layout})` entier
  (`OA/ui/src/components/Diagram/DiagramEditor.jsx:63-85`), statut IN_PROGRESS/SUCCESS/ERROR.
- Autres : recherche textuelle de sommets, vue tableau, export SVG, couleur/rayon par sommet.

### 4.3 Mapping tableur → entités

- Écran en 3 sections : (1) choisir un ou plusieurs schémas cibles, y compris des **schémas-liens**
  (« connections are also treated as entities »), (2) assigner chaque colonne à une propriété et
  choisir les colonnes **clés**, (3) vérifier puis lancer (`OA/docs/user-guide/103/map-data.md`
  étapes 3-5 ; composants `MappingKeyAssign`, `MappingPropertyAssign`, `MappingVerify`,
  `OA/ui/src/components/MappingEditor/MappingEditor.jsx:157-234`). Capture `docs/assets/images/mapping.png`,
  `docs/assets/images/csv_view.png`. Le mapping est exportable et ré-appliquable à un fichier de
  mêmes en-têtes.

### 4.4 Captures

- Publiques : https://openaleph.org/docs/user-guide/103/network-diagrams/ (vidéo YouTube
  `95fkoGkTO-E`), …/investigations/ (vidéo `LPAc1Nf7yqo`), …/map-data/ (vidéo `d_vXhDr40PQ`).
- Dans le dépôt (`OA/docs/assets/images/`) : `new_dia.png`, `add_to_dia.png`, `add_diagram.png`,
  `expand_dia.png`, `mapping.png`, `csv_view.png`, `investigation_settings.png`, `upload_files.png`,
  `xref.png`, `side-bar.png`, `top-bar.png`, `source_docs.png`, `result_preview.png`, `resultlist.png`,
  `share_access.png` (24 images ; **aucune capture de timeline**).
- Issue avec captures du bug « entités fantômes » : https://github.com/alephdata/aleph/issues/2574.

---

## 5. Algorithmes / mécanismes réutilisables

### 5.1 Date à précision par préfixe (FtM + `ImpreciseDate` UI)

```
parse(s): regex ^YYYY(-MM(-DD([T ].*)?)?)?$          # util.ts:19-28 (heure ignorée)
earliest(d) = date(y, m or 1, d or 1)                 # util.ts:244-250
latest(d)   = date(y, m or 12, d or endOfMonth(y,m))  # util.ts:252-261
valid(d)    = y && !(m && d && d > endOfMonth(y,m))   # util.ts:228-242
item.start  = earliest(temporal_start)
item.end    = latest(temporal_end) if valid else latest(temporal_start)   # util.ts:135-141
duration    = days(end - start) + 1 ; singleDay ⇔ duration == 1
```
`temporal_start` = min lexicographique des valeurs des propriétés `temporalExtent.start`, `end` = max
(`proxy.py:356-379`). O(n log n) par entité, trivial. Cas limites : `2019-03` en début = 1er mars,
en fin = 31 mars → une période d'un mois apparaît comme une barre ; « 2019-02-30 » invalide ;
pas de fuseau (UTC imposé) ; pas d'heure dans l'UI (cf. piège 7.1).

### 5.2 Fenêtre et échelle du Gantt

```
earliest = min(item.start), latest = max(item.end) (défaut: aujourd'hui)    # TimelineChart.tsx:40-50
pad = days|months: max(1, 3|12 - |Δmois|) mois ; years: max(1, 3 - |Δans|) ans  # util.ts:22-39
start = startOf(mois|année)(earliest) - pad ; end symétrique
px(item) = (startDay - windowStart) * dayWidth, dayWidth ∈ {20,5,1} px
niveaux dispo : Δ<20 ans → jours, Δ<100 → mois, sinon années          # state.ts:231-253
```
Rendu DOM pur (un `<li>` par élément) : O(n) noeuds DOM, sans virtualisation.

### 5.3 Expansion en deux temps (backend `expand_proxies`)

```
props = propriétés de type entity du schéma (filtrées par ?properties=)     # expand.py:25-35
pour chaque prop "stub" (l'arête est stockée chez l'autre entité) :
    requête ES  properties.<reverse> IN entity_ids, par schéma              # expand.py:71-77
msearch avec comptage (limit=0 ⇒ compteurs seuls)                            # expand.py:79
si limit>0 : graph.resolve() (charge l'extrémité opposée des arêtes)         # expand.py:83-84
pour chaque prop : count, entities = voisins via graph.get_adjacent()        # expand.py:87-108
```
Retour : `[{property, count, entities}]`, propriétés à `count == 0` omises. Complexité : une
msearch (1 requête par couple schéma×prop), plafonnée par `INDEX_EXPAND_CLAUSE_LIMIT`
(`expand.py:22`). Le commentaire admet un format « very awkward » (l.38-42).

### 5.4 Mise en page

- Force : `forceX/forceY` vers le centre, `forceLink(strength 1, distance 10)`, liens de groupement
  `(0.3, 6)`, `forceManyBody(-3)`, **300 ticks synchrones**, `maintainFixed` = sommets épinglés
  restent en place (`fx/fy`) (`RF/…/layout/tools/forceLayout.ts:11-23`, `getForceData.ts:1-58`).
  Groupements : liens complets entre membres → O(k²) par groupe (`getForceData.ts:38-49`).
- Arbre : dagre `ranker: longest-path`, `nodesep 6, edgesep 3, ranksep 9` (`arrangeTree.ts:8-29`).
- Génération (`GraphLayout.generate`, `GraphLayout.ts:106-199`) : marquage `garbage` de tous les
  éléments, puis : entité à schéma edge → arête source→cible par paire ; sinon sommet + arêtes vers
  les valeurs de types pivots activés dans les réglages ; balayage final des `garbage`. **Mark-and-
  sweep** : positions et couleurs des sommets survivants conservées. Recherche des entités par
  `entities.find` → O(n²) (l.120-139).

### 5.5 Mapping rechargeable, idempotent

```
seed = sha1(key_prefix=collection.foreign_id) + key_literal                 # mapping/entity.py:51-52
id(record) = id_column  OU  sha1(seed + sorted(non-vides(keys)))            # entity.py:95-108
map(): pays d'abord (aide au parsing téléphone/adresse), puis le reste ;
       valeurs non nettoyables loguées et rejetées ; required manquant ⇒ ligne ignorée  # l.110-163
propriété: column(s) | literal(s) | template "{col}" | join | split | format | fuzzy  # property.py:48-57
load_mapping(): delete(origin="mapping:<id>") ; map ; entity.add("proof", table_id) ;
                put(origin) ; option: ajoute à l'entityset ; status SUCCESS/FAILED   # logic/mapping.py:45-104
```
Déterministe : relancer = mêmes IDs, donc pas de doublons ; supprimer la source = supprimer tout ce
qui a son origine.

---

## 6. Trois patterns à reprendre dans PNEX

### 6.1 Dates à précision explicite + bornes basse/haute
- **Quoi** : stocker le temps d'un événement comme (valeur, précision) — `2019-03` ≠ `2019-03-01` —
  et dériver `earliest`/`latest` pour l'affichage (barre = intervalle d'incertitude).
- **Pourquoi** : journaliste/vidéaste true-crime ont des sources « mars 2019 » ; l'ingénieur IoT a la
  seconde. Un seul modèle couvre les deux sans faux précis.
- **Comment** : dans `objects`/événements, colonnes `t_start timestamptz`, `t_start_precision
  smallint` (année…seconde), idem fin ; `tstzrange` généré `[earliest, latest)` + index GiST pour
  « le dossier à HH:MM » (`@>`). Côté Rust, un `enum Precision` et une fonction `bounds()` testée
  (fin de mois, bissextile). Côté Dioxus, la frise dessine en hachuré la zone d'incertitude au lieu
  d'un point. Supporter l'heure dès le départ (≠ Aleph).

### 6.2 `temporalExtent` déclaratif par type + « edge schema » séparé du rôle
- **Quoi** : chaque type d'objet déclare *quelles* propriétés donnent son début/fin (Person :
  naissance/décès, Device : mise en service/retrait, Media : date de prise) ; les relations
  binaires déclarent `source/target/label/directed`. FtM rappelle qu'**un intervalle n'est pas une
  arête** (`Interval.yaml:2-11`).
- **Pourquoi** : la frise PNEX peut afficher *n'importe quel* objet attaché sans code spécifique, et
  le graphe sait dessiner une attache (événement → objet, rôle) comme arête étiquetée, tandis qu'un
  événement à N participants reste un **nœud** (hyper-arête), comme Event/involved.
- **Comment** : table de métadonnées de type (ou const Rust dans le catalogue d'ontologie) `temporal_start: &[&str]`,
  `temporal_end`, `edge: Option<EdgeSpec{source,target,label,directed}>` ; un test de garde « tout
  type a un extent ou est marqué non datable ». Le rôle (preuve/acteur/contexte/hypothèse/source)
  reste une colonne de `resource_edges` (comme `Interest.role`), pas un sous-type — plus souple que
  `organizer`/`involved` figés de FtM.

### 6.3 Expansion de nœud en deux temps, annulable, avec sélection du résultat
- **Quoi** : double-clic → menu avec **compteurs par type de lien** (requête « count only ») →
  l'utilisateur choisit → chargement, layout incrémental (nœuds épinglés fixes), nouveaux nœuds
  sélectionnés, toast « +N nœuds, +M liens », entrée d'historique pour rollback.
- **Pourquoi** : évite l'explosion du graphe (un hub à 3 000 voisins), rend l'action réversible —
  exactement l'« expansion avec rollback » du PRD.
- **Comment** : endpoint `GET /api/v1/objects/{id}/neighbors?count_only=true` → `SELECT edge_kind,
  role, count(*) … GROUP BY` sur `resource_edges` filtré `valid_from/valid_to` à la fenêtre de la
  frise (estompage temporel) ; puis `?kind=…&limit=`. Côté front, pile `Vec<Snapshot{layout,
  added_ids}>` dans un `Signal` (undo = restaurer layout + retirer les ids ajoutés, aucun appel
  serveur si l'expansion n'est qu'une vue). Org depuis le principal (R1).

(Bonus non retenu dans les 3 : mapping CSV idempotent avec `origin = "mapping:<id>"` et
`proof = table` — à copier tel quel pour l'import de traces CSV/GPX, cf. §8a.)

---

## 7. Trois pièges à éviter

### 7.1 Frise plafonnée au jour, heures ignorées
- **Problème** : la regex UI ignore l'heure (« we accept datetime values, but simply ignore and do
  not validate them », `OA/ui/src/components/Timeline/util.ts:23-24`) ; tout est en jours
  (`differenceInDays`, plus petite unité 20 px/jour). Un datetime faisait même planter la vue Gantt.
- **Preuve** : code cité ; issue https://github.com/alephdata/aleph/issues/3009 (« Page turns blank
  due to an unhandled date parsing error »), PR #3014 ; demande de zooms 5/10 ans #3222.
- **Parade PNEX** : échelle temporelle continue en millisecondes (zoom molette de l'année à la
  seconde), un seul parseur Rust partagé backend/wasm, tests sur valeurs partielles et datetimes.
  Pour la RCA IoT, la seconde est le cas nominal.

### 7.2 Layout JSON monolithique, last-write-wins, membres via un modèle détourné
- **Problème** : diagramme et frise enregistrent le layout **entier** à chaque modification
  (`DiagramEditor.jsx:63-85`, `TimelineScreen.jsx:140-150`) sans version : deux éditeurs simultanés
  s'écrasent. L'appartenance passe par `EntitySetItem.judgement`, hérité des profils de
  dédoublonnage ; « retirer » = `no_judgement` (commentaire « The API … is weird »,
  `TimelineScreen.jsx:127-133`).
- **Preuve** : issue https://github.com/alephdata/aleph/issues/2453 (« when several people modify a
  diagram simultaneously, the page must be reloaded »).
- **Parade PNEX** : attachements = lignes `resource_edges` (rôle, valid_from/to) avec concurrence
  optimiste (`expected_version` → 409, déjà la doctrine PNEX) ; positions de nœuds/couleurs dans une
  table de layout par vue, patchs unitaires ; membres déduits des attachements (PRD), jamais une
  liste parallèle.

### 7.3 Incohérences de « datable » et arêtes fantômes
- **Problème** : (a) la page entité n'autorise l'ajout à une frise que pour `isA('Interval')`
  (`EntityScreen.jsx:131`) alors que le tableau utilise « a une propriété de début temporelle »
  (`EntityTable.jsx:275`) : une Person est ajoutable depuis l'un, pas depuis l'autre. (b) Ajouter
  un intervalle-lien à un diagramme sans ses extrémités crée des **entités fantômes** sans schéma
  (`GraphLayout.generate` fabrique des sommets à partir de références non chargées,
  `GraphLayout.ts:118-152`). (c) Dans le Gantt, les éléments sans date disparaissent silencieusement
  (`TimelineChart.tsx:23-25`) alors que la liste les montre en tête. (d) `generate` en O(n²)
  (`entities.find` dans des boucles).
- **Preuve** : code cité ; issue https://github.com/alephdata/aleph/issues/2574 (captures).
- **Parade PNEX** : une seule fonction `is_datable(kind)` côté core utilisée partout ; à l'ajout
  d'une relation, charger ou proposer ses extrémités ; les objets non datés vont **visiblement** dans
  le « bac des pièces non datées » du PRD ; index `HashMap<id, Node>` dans la génération du graphe.

---

## 8. Réponses aux questions ouvertes du PRD

**(a) Traces importées CSV/GPX vs flux.** Aleph ne connaît pas de séries temporelles : un tableur
devient des **entités** via un mapping persistant, rejouable et idempotent (IDs = sha1 des clés),
avec `origin="mapping:<id>"` et `proof=table_id` (`logic/mapping.py:45-72`). Transposé : un import
CSV/GPX ne doit pas copier les points dans Postgres comme objets — garder le fichier comme média
(preuve) et, soit l'**ingérer une fois dans O2** sous un stream marqué `origin=import:<media_id>`
(supprimable/rejouable comme Aleph), soit le lire à la volée ; dans les deux cas la trace est
servie au même format kepler que les flux live, et l'événement/objet porte `proof → média`. Le
mapping de colonnes (lat/lon/ts/alt, format de date, clé d'acteur) mérite d'être sauvegardé et
réappliquable comme dans Aleph. Aleph ne tranche pas sur l'interpolation (inexistante chez lui).

**(b) Graphe natif vs Cytoscape.js en wasm.** Aleph/react-ftm a choisi un **rendu SVG maison**
(+ d3-force 300 ticks synchrones, dagre pour les arbres, pan/zoom faits main, `Canvas.tsx`) ; il
couvre sélection, groupements, dessin d'arête, export SVG, undo, mais la lib a été abandonnée
(archivée 2022) et souffre de dette (O(n²), arêtes fantômes). Pour PNEX : un SVG rendu par Dioxus
+ force-directed calculé en Rust (quelques centaines de nœuds pour un dossier) est faisable et
cohérent avec la doctrine « pas de JS tiers » ; le volume d'un dossier d'enquête reste du même
ordre que les diagrammes Aleph (quelques centaines d'éléments). Cytoscape n'apporte un gain
qu'au-delà (milliers de nœuds, layouts avancés). Aleph montre que le natif suffit pour le cas
d'usage, à condition d'un index par id et d'un layout incrémental qui respecte les nœuds épinglés.

**(c) Nommage.** Aleph code `casefile` mais affiche **« Investigations »** (`collection.py:56`) ;
la doc dit « Investigation … a secure, private environment » et « Building structured case files
over time ». Les sous-objets s'appellent « Network diagram », « Timeline », « List ». Pour PNEX en
français : « **Enquête** » (court, verbe d'action, symétrique d'« Investigation ») pour l'écran ;
« dossier » pour l'objet exporté/gelé (« dossier d'enquête » = snapshot). « Projet » est trop
générique (collision avec la gestion de projet).

---

## 9. Sources

Fichiers lus (OpenAleph `82fed2e`) :
- `OA/LICENSE.txt`, `OA/pyproject.toml`, `OA/requirements.txt`, `OA/ui/package.json`, `OA/mkdocs.yml`
- `OA/aleph/model/collection.py`, `OA/aleph/model/entityset.py`, `OA/aleph/model/entity.py`,
  `OA/aleph/model/mapping.py`, `OA/aleph/logic/mapping.py`, `OA/aleph/logic/expand.py`
- `OA/ui/src/components/Timeline/{Timeline.tsx,state.ts,util.ts,types.ts,TimelineItemCreateForm.tsx,TimelineActions.tsx,TimelineZoomLevelSwitch.tsx}`
- `OA/ui/src/components/Timeline/TimelineChart/{TimelineChart.tsx,TimelineChartItem.tsx,util.ts,TimelineChart.scss}`,
  `OA/ui/src/components/Timeline/TimelineList/TimelineListItem.tsx`
- `OA/ui/src/screens/TimelineScreen/TimelineScreen.jsx`, `OA/ui/src/screens/EntityScreen/EntityScreen.jsx`,
  `OA/ui/src/components/EntityTable/EntityTable.jsx`, `OA/ui/src/components/EntitySet/EntitySetSelector.jsx`,
  `OA/ui/src/components/Diagram/DiagramEditor.jsx`, `OA/ui/src/components/MappingEditor/MappingEditor.jsx`,
  `OA/ui/src/queries.js`
- `RF/components/NetworkDiagram/{NetworkDiagram.tsx,History.ts}`, `…/layout/{GraphLayout.ts,Vertex.ts,Edge.ts}`,
  `…/layout/tools/{forceLayout.ts,getForceData.ts,arrangeTree.ts}`, `…/renderer/{Canvas.tsx,VertexRenderer.tsx}`,
  `…/toolbox/Toolbar.tsx`, `RF/components/common/EntityManager.ts`
- `OA/docs/user-guide/103/{investigations.md,network-diagrams.md,map-data.md}`, `OA/docs/assets/images/expand_dia.png`

Fichiers lus (FollowTheMoney `v4.11.0`) :
- `FTM/LICENSE`, `FTM/pyproject.toml`, `FTM/js/package.json`
- `FTM/followthemoney/schema/{Interval,Event,Interest,Ownership,Membership,Thing,Person,LegalEntity,Document}.yaml`
- `FTM/followthemoney/types/date.py`, `schema.py`, `proxy.py`, `graph.py`, `statement/statement.py`,
  `mapping/entity.py`, `mapping/property.py`

URLs :
- https://github.com/alephdata/react-ftm (archivé 29/11/2022) et
  https://raw.githubusercontent.com/alephdata/react-ftm/master/package.json (licence MIT)
- https://openaleph.org/docs/user-guide/103/investigations/ , …/network-diagrams/ , …/map-data/
- https://github.com/alephdata/aleph/issues/2673 (feuille de route Timelines), /issues/3009 (datetime),
  /issues/2574 (entités fantômes), /issues/2453 (édition collaborative), /issues/3222 (zooms),
  /pull/3005 (ajout d'entités existantes), /pull/2946 (vue chart), /pull/3751 (retrait des embeds de diagrammes)
- (404) docs.aleph.occrp.org/users/investigations/timelines — doc timeline de l'ancien Aleph introuvable ;
  aucune capture publique de timeline trouvée.
