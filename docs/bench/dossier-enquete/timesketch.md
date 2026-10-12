# Fiche benchmark — Timesketch (Google)

> Racine étudiée : `bench/timesketch` (abrégé `TS/`). Les chemins
> `timesketch/...` sont relatifs à cette racine. Rien n'a été compilé ni exécuté.

## 1. Identité

- **Dépôt** : https://github.com/google/timesketch — commit étudié `f0b3602` (« reuse existing sketch if name is given (#3868) »).
- **Licence** : **Apache-2.0**, confirmée par `LICENSE` (texte « Apache License Version 2.0, January 2004 ») et par les en-têtes :
  171 fichiers Python sous `timesketch/` portent « Licensed under the Apache License » ; les `.vue` aussi
  (ex. `timesketch/frontend-v3/src/components/Explore/EventList.vue:1-15`, « Copyright 2025 Google Inc. »).
  Un seul fichier `LICENSE` dans l'arbre (hors `node_modules` non cloné) : **pas d'édition entreprise, pas de
  sous-dossier sous autre licence**. Dépendances notables côté analyseurs : base GeoLite2 MaxMind (licence
  propre, téléchargée à part, `timesketch/lib/analyzers/geoip.py:16`).
- **Ce que ça permet à PNEX (MIT)** : Apache-2.0 est compatible avec une redistribution sous MIT **pour le
  code repris**, à condition de conserver la notice de copyright + la licence Apache, de signaler les
  modifications et de reprendre un éventuel fichier NOTICE (absent ici). Reprise de code possible avec
  attribution ; en pratique on est en Python/Vue → **inspiration** (modèle, flux) plutôt que copie. Le brevet
  implicite Apache protège même un portage.

## 2. Ce que fait le projet

Timesketch est un outil d'analyse collaborative de **frises chronologiques forensiques** : on importe des
timelines (Plaso, CSV, JSONL) dans OpenSearch, on les regroupe dans un **sketch** (= le dossier), on cherche,
filtre par temps, **étoile / tague / commente** des événements, on lance des **analyzers** qui enrichissent
les événements, puis on rédige des **stories** et, depuis 2024-2025, on répond à des **questions
d'investigation (DFIQ)** avec des **conclusions** étayées par des événements-« faits ».
**Nous concerne** : modèle d'annotation collaborative sur des événements bruts immuables, chemin
« événement brut → étoile → fait rattaché à une conclusion → rapport vérifié », histogramme cliquable,
recherche de contexte ±N, arbre d'historique de recherche, analyseurs (et leurs dérives « imposent »).

## 3. Modèle de données pertinent

Deux magasins : **PostgreSQL/SQLAlchemy** pour le métier (sketch, annotations riches) et **OpenSearch**
pour les événements bruts (un index par import) — les annotations « légères » (étoile, tag) sont
**écrites dans le document OpenSearch**.

### 3.1 Conteneurs
- `Sketch` (`timesketch/models/sketch.py:49`) : `name`, `description`, `user_id`, mixins ACL/Label/Status/Comment.
  Relations : `timelines`, `views`, `events`, `stories`, `aggregations`, `attributes`, `graphs`,
  `analysis`, `searchhistories`, `scenarios`, `facets`, `questions` (`:59-143`). = notre « case ».
- `SearchIndex` (`sketch.py:284`) : l'index OpenSearch (`index_name`), **partageable entre sketches** (ACL propre).
- `Timeline` (`sketch.py:267`) : vue d'un SearchIndex dans un sketch ; `color` (6 hex, aléatoire par défaut
  `:272`) — c'est la couleur de ligne/pastille par source. `DataSource` (`sketch.py:531`) : provenance de
  l'import (`provider`, `context`, `original_filename`, `file_size`, `data_label`, `error_message`,
  `total_file_events`).

### 3.2 L'événement
- **Brut** : document OpenSearch. Champs obligatoires `message`, `datetime`, `timestamp_desc`
  (`timesketch/lib/utils.py:46`, `TIMESKETCH_FIELDS`), plus `timestamp` (µs epoch, recalculé à l'import
  `utils.py:446-451`), `__ts_timeline_id`, `tag` (liste), `__ts_emojis`, `human_readable`, `timesketch_label`
  (nested, `opensearch.py:1723` mapping `{"timesketch_label": {"type": "nested"}, "datetime": {"type":"date"}}`).
- **Temps** : **instant unique** par document. Pas de période, pas d'incertitude. La « durée » n'existe que
  via un champ libre ou en dupliquant (un événement par `timestamp_desc` : « Creation Time », « Last Access »…
  — sémantique héritée de Plaso). `timestamp_desc` = **le rôle du temps**, pas de l'objet.
- **Ancre SQL** `Event` (`sketch.py:412-417`) : `sketch_id`, `searchindex_id`, `document_id`. Créée
  paresseusement (`Event.get_or_create`, `timesketch/api/v1/resources/event.py:1063`) **uniquement quand on
  annote** : c'est la jonction brut ↔ métier. Un même document a un `Event` par sketch.

### 3.3 Annotations (mixins génériques, `timesketch/models/annotations.py`)
Chaque mixin crée dynamiquement une table `<parent>_<type>` avec `parent_id` + `user_id` + horodatage
(`BaseModel.created_at/updated_at`, `timesketch/models/__init__.py:78-79`) :
- `Label` (`annotations.py:60`, champ `label` 255) — `LabelMixin` `:87` ; utilisé pour labels système
  préfixés `__ts_` : `__ts_star`, `__ts_comment`, `__ts_hidden`, `__ts_fact_<conclusion_id>`,
  `__ts_priority_<lvl>` (questions).
- `Comment` (`annotations.py:66`) — `CommentMixin` `:198` (édition/suppression : auteur seul, vérifié serveur
  `event.py:1248` et `:1323`).
- `Status` (`annotations.py:72`) — `StatusMixin.set_status` `:387` garantit un seul statut (race testée
  `models/status_race_test.py`).
- `GenericAttribute` (`annotations.py:78`) : `name`, `value`, `ontology`, `description` — attributs typés
  libres sur Scenario/Facet/Question/Analysis (ex. `source=AI_GENERATED`).

**Double écriture** pour étoile/commentaire/label : ligne SQL (`event.labels.append`, `event.py:1093`) **et**
entrée `{"name", "user_id", "sketch_id"}` dans `timesketch_label` du document (`set_label`,
`timesketch/lib/datastores/opensearch.py:1641`, scripts painless `:81-103`). À la lecture, l'API aplatit en
`_source.label` en ne gardant que les labels du sketch courant (`timesketch/api/v1/resources/explore.py:366-378`).
Les **tags** (`tag`) et **attributs** d'événement (`EventAddAttributeResource`, `event.py:356`, max 10 attrs
× 100 000 événements, noms réservés `datetime/timestamp/message/timestamp_desc` `:362-367`) sont écrits
**directement dans le document**, **sans sketch, sans auteur, sans date** (`_tag_event`, `event.py:59-97`).

### 3.4 Recherches et récit
- `View` = « saved search » (`sketch.py:336`) : `query_string`, `query_filter` (JSON : chips, indices, ordre,
  taille), `query_dsl`, lien optionnel `SearchTemplate` (`:395`, réutilisable entre sketches, `template_uuid`).
- `SearchHistory` (`sketch.py:588`) : **arbre** (`parent_id` auto-référent `:592`, `children` `:606`),
  `query_*`, `query_result_count`, `query_time`, lié à `scenario/facet/question/approach` et à des `events`
  (table d'association `:580`) ; les labels du nœud (`__ts_star`, `__ts_comment`…) marquent « dans cette
  recherche j'ai étoilé/commenté » (`event.py:1084`, `:1123-1133`).
- `Story` (`sketch.py:420`) : `title`, `content` = **JSON d'une liste de blocs**
  `{componentName, componentProps, content(markdown), edit, …}` (`timesketch/lib/analyzers/interface.py:746-760`).
  Un bloc peut embarquer une View (`TsEventList`), une agrégation, un graphe, une question.
- `Graph` (`sketch.py:506`) : `graph_elements` en JSON Cytoscape + vignette.

### 3.5 Investigation (DFIQ) — le chemin « brut → fait retenu »
- `Scenario` (`sketch.py:659`) → `Facet` (`:763`, avec `FacetTimeFrame` `:683` : `start_time`/`end_time`
  **TIMESTAMP tz** = seule vraie période du modèle) → `InvestigativeQuestion` (`:883`) →
  `InvestigativeQuestionApproach` (`:940`, liée à SearchTemplates et règles Sigma).
- `InvestigativeQuestionConclusion` (`sketch.py:846`) : `conclusion` (texte), **`automated` (bool)** `:857`,
  `user_id`, et des M2M vers `stories`, `saved_searches`, `saved_graphs`, `saved_aggregations`, `analysis`
  et **`events`** (`:834-879`). Un événement relié = un **fait** ; côté document il porte
  `__ts_fact_<conclusion_id>` (`event.py:1088-1091`). Garde anti-fuite : la conclusion doit appartenir au
  sketch de l'URL (`event.py:1143-1150`).
- Statut des questions (StatusMixin) : `new` → `pending-review` → `verified` | `rejected`
  (`timesketch/frontend-v3/src/components/Investigation/ReportView/QuestionActionsStrip.vue:209-221`).
- **Rapport** : une Story spéciale (`title="__ts_investigation_report"`,
  `timesketch/lib/analyzers/llm_log_analyzer.py:129-141`) dont `content` est un objet JSON
  (`summary`, `status`, `approvedQuestions`, `removedQuestions`, `conclusionSummaries[]`, `analysts`,
  `completedDateTime`) ; `status ∈ {VERIFIED, UNVERIFIED}` (`timesketch/frontend-v3/src/stores/app.js:21-24`).

### 3.6 Provenance / versionnage
- Provenance d'import : `DataSource`. Provenance d'analyse : `Analysis` (`sketch.py:460` : `analyzer_name`,
  `parameters`, `result`, `log`, lien `approach_id`/`question_conclusion_id`) + `AnalyzerOutput`
  normalisé JSON-Schema (`interface.py:1211-1300` : `result_status`, `result_priority` NOTE/LOW/MEDIUM/HIGH,
  `created_tags`, `created_attributes`, `saved_views`…).
- **Aucun versionnage** des Stories, Views, commentaires (écrasement, `timesketch/api/v1/resources/story.py:237-239`).
  Seul « historique » : la liste `conclusionSummaries` préfixée côté client (cf. §4.8).

## 4. Patterns UX observés

UI par défaut = **frontend-ng** (Vue 2/Vuetify) ; **frontend-v3** (Vue 3) servi si `v3_ui`, et
`frontend/` = legacy (`timesketch/app.py:66-74`). Le module « Investigation » n'existe qu'en v3.

### 4.1 Liste d'événements (Explore)
`timesketch/frontend-ng/src/components/Explore/EventList.vue` : tableau paginé serveur (10…500 lignes),
colonne datetime colorée par timeline, colonnes champ libres. Par ligne : **bouton étoile** (`:423-424`,
ambre si `__ts_star`), menu tags (`EventTagMenu.vue`), menu d'actions (`EventActionMenu.vue`), pastilles
tags + emojis d'analyseurs dans la 1ʳᵉ colonne (`:449-480`), compteur de commentaires (`:497`). Clic sur la
ligne = **détail déplié** (`EventDetail.vue`) : tous les attributs + fil de commentaires (`Comments.vue` :
initiale de l'auteur, date relative, édition/suppression si auteur `:56`). Étoile = **optimiste** :
la liste locale est modifiée puis `saveEventAnnotation(…'label','__ts_star'…, currentSearchNode)`
(`EventList.vue:1166-1177`). Sélection multiple → actions de masse (tags, étoile) ; doc
`docs/assets/images/bulk_edit.png`.

### 4.2 Étiquetage rapide
Trois **quick tags** codés en dur `bad` (rouge), `suspicious` (orange), `good` (vert)
(`EventTagMenu.vue:37-41`, `EventTagDialog.vue:102`), toujours triés en tête (`EventTags.vue:43-56`,
« TODO: refactor when quickTags become configurable »). Tags libres en autocomplétion.

### 4.3 Histogramme temporel
Bouton « Toggle event histogram » dans la barre d'outils de la liste (`EventList.vue:192-193`), carte
dépliable (`:376-389`) avec bouton **reset** des filtres temps. Composant
`timesketch/frontend-ng/src/components/Explore/BarChart.vue` (ApexCharts, 250 px, barres 95 %). Données =
agrégat `auto_date_histogram` **50 buckets** sur `datetime`, calculé **dans la même requête** que la page
d'événements (`timesketch/api/v1/resources/explore.py:226-231`, extraction `:312-322`). Tooltip =
« début du bucket + durée humanisée » (`BarChart.vue:72-78`). **Clic sur une barre** → chip
`datetime_range` `[début, début+intervalle]` (`emitFilterRequest`, `BarChart.vue:136-157`) qui **remplace**
les autres chips temporels (`addChipFromHistogram`, `timesketch/frontend-ng/src/views/Explore.vue:670-679`).
Zoom/sélection Apex activés dans la toolbar mais la sélection de plage au glisser n'émet rien : seul le clic
de barre filtre. Pas de mini-frise par timeline.

### 4.4 Filtres de temps = chips
`Explore.vue:163-247` : chaque plage est un **chip** (icône horloge, « A → B ») avec menu : éditer
(`FilterMenu.vue`), copier, **désactiver temporairement** (barré à 50 % d'opacité, `:170`), supprimer. Les
chips temporels multiples sont combinés en **OR** (bouton « OR » entre chips `:224`). Capture :
`docs/assets/images/timefilter.png`, `removable_chip_filters.png`, `filters.png`.

### 4.5 Recherche de contexte ±N
Menu d'action « context » (`EventActionMenu.vue:35`, `:84-85` émet `showContextWindow`). Implémentation
`searchContext` (`Explore.vue:557-599`) : sauvegarde la requête courante (`originalContext`), remplace la
requête par `* OR _id:<id>` sur **la seule timeline de l'événement**, deux chips `[t-300 s, t]` et
`[t, t+300 s]`, taille 500 ; bandeau « Showing context for event » (`EventList.vue:105`) ; `removeContext`
restaure la recherche d'origine (`:601-606`). **Fenêtre fixe 300 s / 500 événements** (« TODO: Make this
selectable in the UI », `:558`). Capture : `docs/assets/images/context_search.png`.

### 4.6 Historique de recherche en arbre
`SearchHistoryTree.vue` / `SearchHistoryTreeNode.vue` : chaque recherche = nœud enfant de la précédente,
branche quand on revient en arrière ; les nœuds portent les icônes des actions faites (étoile, commentaire,
label) grâce aux labels posés en §3.4. Doc : « tree with support for branching, annotations and intuitive
navigation », vidéo `docs/assets/videos/SearchHistory.mp4`
(https://timesketch.org/guides/user/sketch-overview/).

### 4.7 Stories
`timesketch/frontend-ng/src/views/Story.vue` : document de blocs ; entre deux blocs, bouton « + » pour
insérer texte markdown, saved search, graphe, agrégation, question (`:236+`, `addEventListBlock` `:505-512`).
Un bloc saved search affiche **les 10 premiers résultats en direct** de la View (`formatComponentProps`
`:468-495` : `size=10`, `incognito`, `disableSaveSearch`) + lien « ouvrir dans Explore ». Ce n'est **pas un
instantané** : si les données ou la View changent, la story change. Rendu markdown via
`DOMPurify.sanitize(marked(...))` (`:497`). Sauvegarde = PUT du JSON complet (`save`, `:580-600`).

### 4.8 Investigation (frontend-v3)
`timesketch/frontend-v3/src/components/Investigation/` : barre latérale de questions (filtres statut /
priorité via labels `__ts_priority_*`, `Sidebar/FilterBar.vue:182`), vue Rapport, vue Résultats.
- **Lier des faits** : tiroir « Link Supporting Events » / « Add Additional Supporting Events »
  (`EventsLog/index.vue`) = `EventList` réduit (pas d'histogramme, pas d'étoile ni de tag,
  `disableIQFacts=false`) ; une icône par ligne bascule `__ts_fact_<conclusionId>`
  (`frontend-v3/src/components/Explore/EventList.vue:441-444`, `toggleFact` `:1255-1280`). En mode brouillon
  (conclusion pas encore créée) la sélection est locale (`isDraftMode`, label générique `__ts_fact`).
- Chaque fait s'affiche dans la conclusion (`EventsLog/ConclusionFact.vue`) : date + `timestamp_desc` +
  message, chargés à la demande (`ApiClient.getEvent`, `:102`), loupe pour le détail.
- **Cycle de revue** : boutons « Not Relevant » (→ `rejected`), « Review Conclusions », « Re-open for
  Review » (→ `pending-review`, avec dialogue d'avertissement `QuestionActionsStrip.vue:186-188`).
  À la **vérification**, si le dernier résumé est signé `<AI>`, il est **re-signé par l'analyste** (copie
  préfixée, `:288-300`) et l'analyste est ajouté à `analysts` (`:303-310`).
- **Gel** : « Complete investigation » → `status=VERIFIED`, `completedDateTime` (`ReportView/index.vue:327-333`,
  `Modals/CompleteInvestigationModal.vue:89`) ; `reportLocked` désactive les boutons
  (`stores/app.js:341-343`) ; « unlock » repasse `UNVERIFIED` (`ReportView/index.vue:415-419`) ; export PDF
  (`_utils/pdf-generator.js`). « View History » des résumés (`ReportView/SummarySection.vue:26-31`).

### 4.9 Ajout manuel d'événement
`AddManualEvent.vue` → `EventCreateResource` (`event.py:100-237`) : date (parse `dateutil`, défaut =
maintenant), `message`, `timestamp_desc` (« Event Happened »), attributs dict, tags ; versé dans une
timeline interne **« Manual events »** par sketch (index md5 de `timesketch_<id>`). Capture
`docs/assets/images/add_event.png`.

### 4.10 Captures présentes dans le dépôt (`docs/assets/images/`)
`context_search.png`, `timefilter.png`, `removable_chip_filters.png`, `filters.png`, `bulk_edit.png`,
`singleevent.png`, `2events.png`, `add_event.png`, `heatmap.png`, `screenshot-202212-dark.png`,
`timelinenavigation1.png`, `Navigation.png`, `empty_state_sketch.png`, `Newsketch.png`, `Import.png`,
`AI-Exploration-Graph.jpg`, `CoinMinerKillChain.jpg`, `Sharingdialog.png`, `add_intelligence.gif`,
`Sigma_create_rule_new.gif`, `TS_ContextLinks_RedirectWarn.gif` ; vidéo `docs/assets/videos/SearchHistory.mp4`.
Doc publique : https://timesketch.org/guides/user/sketch-overview/ (timefilter, navigation, partage).

## 5. Algorithmes / mécanismes réutilisables

### 5.1 Sessionizer (regroupement temporel) — `timesketch/lib/analyzers/sessionizer.py`
```
max_gap = 300_000_000 µs (5 min, codé en dur, « TODO configurable »)
events = stream(query, tri par timestamp asc)
session = 1 ; annoter(e0, 1) ; last = e0.ts
pour e dans events : si e.ts - last > max_gap : session += 1
                     annoter(e, session) ; last = e.ts
annoter : e.session_id[session_type] = session  (dict → plusieurs sessionizers cohabitent)
```
O(n) en un passage streamé. Cas limites : écart exactement = max_gap reste dans la session (`>` strict) ;
ordre global toutes timelines confondues (pas par acteur) ; résultat **écrit dans les données brutes**.
Transposition PNEX : « fin de signal » = trou > seuil dans la série d'un device ; **par acteur**.

### 5.2 Recherche de contexte (`Explore.vue:557-598`)
`fenêtre = [t-Δ, t+Δ]`, Δ=300 s, limite 500, filtrée sur la timeline source, requête `* OR _id:X` pour garantir
la présence de l'événement pivot ; restauration exacte de l'état précédent. Simple, O(requête).

### 5.3 Histogramme auto (`explore.py:226-231`)
`auto_date_histogram(field=datetime, buckets=50)` : OpenSearch choisit l'intervalle (1s…1y) pour ≤ 50
barres ; renvoie `interval` (« 3h », « 1d ») que le client parse (`BarChart.vue:124-134` : regex `/(\d+)/`
→ count + unité moment.js). Clic = `[key, key+interval]`. Coût : agrégat sur tous les hits à chaque page.
Cas limite : bucket de clé `0` ignoré (`if key:` `explore.py:320`) ; un seul bucket → clic sans effet
(`BarChart.vue:141-143`).

### 5.4 Similarity scorer — `timesketch/lib/analyzers/similarity_scorer.py`, `timesketch/lib/similarity.py:108-125`
MinHash LSH (datasketch) par `data_type`, `threshold=0.5`, `num_perm=128` ; `score = |voisins(e)| / N` ∈ [0,1]
écrit dans `similarity_score`. Marqué « Experimental ». O(n·num_perm).

### 5.5 Chain analyzer — `timesketch/lib/analyzers/chain.py`
Plugins (`chain_plugins/`) : requête de base → `process_chain(event)` → `build_chain(base, chain_id=uuid4)` ;
chaque événement reçoit `chains: [{chain_id, plugin, is_base, leafs}]` + emoji LINK (`chain.py:117-123`).
= **lien n-aire entre événements** par UUID, sans table de relation.

### 5.6 Import CSV/JSONL — `timesketch/lib/utils.py`
- Obligatoires `message, datetime, timestamp_desc` ; **mapping d'en-têtes** dans l'UI (renommer une colonne,
  créer une colonne à valeur par défaut, concaténer plusieurs colonnes en `message`)
  (`docs/guides/user/import-from-json-csv.md`).
- `datetime` numérique > 1e15 → µs (`:405-414`) ; sinon `to_datetime(format="mixed", utc=True)` (`:420`) —
  **naïf = UTC** ; années < 1700 ou > 9999 → NaT (`:424-428`) ; **lignes NaT supprimées avec un simple
  warning de log** (`:430-440`). Epochs : heuristique de magnitude ns/µs/ms/s (`:145-152`).
- `timestamp` recalculé en µs (`:446-451`) ; champs > 32 766 octets : texte seulement, pas d'agrégat/tri.

### 5.7 Analyzers : proposent ou imposent ?
- API `interface.Event` : `add_tags`, `add_attributes`, `add_emojis`, `add_star` (→ `__ts_star`, user_id 0),
  `add_comment` (user `None`), `add_human_readable("[analyzer] …")` (`interface.py:143-317`). Tout est
  **écrit d'office** dans le document ou le sketch. Ex. geoip : attributs + drapeau emoji + **tag pays**
  (`geoip.py:332-339`).
- Traçabilité minimale : `AnalyzerOutput.created_tags/created_attributes` (`interface.py:1447-1463`), préfixe
  `[analyzer]` dans `human_readable`, emoji. **Pas** d'auteur par tag, pas d'annulation par run.
- Exception récente « propose » : le LLM log analyzer crée des questions statut **`pending-review`**,
  attribut `source=AI_GENERATED`, conclusions `automated=True`, `user=None`
  (`timesketch/lib/llms/features/log_analyzer.py:515-552`) — l'humain vérifie ou rejette (§4.8). Idem
  conclusions DFIQ `automated=True` (`timesketch/lib/tasks.py:544-551`).

## 6. Trois patterns à reprendre dans PNEX

### P1 — Ancre paresseuse « brut immuable ↔ annotation » + double index de filtrage
- **Quoi** : l'événement brut reste dans son magasin (OpenSearch) ; on crée une ligne SQL `Event(sketch,
  index, document_id)` seulement quand quelqu'un l'annote ; les annotations sont des tables typées
  (auteur, date) ; un **marqueur dénormalisé** dans le doc permet de filtrer `label:__ts_star` côté moteur.
- **Pourquoi** : on peut étoiler/commenter un point de mesure O2 ou une position GPS sans le copier, et
  filtrer vite « ce qui est retenu ».
- **Comment** : table `case_event_refs(case_id, source_kind ['o2_stream'|'object'|'media'], source_ref
  [stream + _timestamp + hash], first_annotated_by, created_at)` créée à la première annotation ;
  annotations dans `object_annotations(kind, author_id, body, created_at)` rattachées à cette ancre (ou
  à l'objet `objects` universel). Pas de marqueur dans O2 (immuable, append-only) : le filtrage
  « retenus seulement » se fait par jointure côté Postgres sur la fenêtre affichée, puis le front
  Dioxus surligne. **Scoper par dossier** (contrairement aux tags TS, cf. piège 2).

### P2 — Fait = lien typé événement → conclusion, avec cycle de revue et signature humaine
- **Quoi** : `InvestigativeQuestionConclusion.events` (M2M) + `automated` + statut
  `new/pending-review/verified/rejected` ; à la vérification, le texte IA est re-signé par l'humain
  (`QuestionActionsStrip.vue:288-310`).
- **Pourquoi** : c'est exactement notre « détecteurs toujours PROPOSÉS » + « rôle à l'attache » :
  une proposition reste distinguable d'un fait retenu, et qui a validé est tracé.
- **Comment** : sur `resource_edges` (attache objet → événement de dossier), ajouter `role`
  (preuve/acteur/contexte/hypothèse/source), `state` (`proposed|accepted|rejected`), `proposed_by`
  (`detector:<nom>@<version>` ou user), `accepted_by/at`. Les détecteurs Rust (co-présence, geofence,
  arrêts) n'insèrent que des `proposed` ; l'inspecteur Dioxus affiche une file « à revoir » avec
  accepter/rejeter (Échap = rejeter, Entrée = accepter). Rejet conservé (pas supprimé) pour ne pas
  re-proposer au run suivant (clé d'idempotence = hash(détecteur, objets, fenêtre)).

### P3 — Histogramme cliquable + chips de plage désactivables + contexte ±Δ réversible
- **Quoi** : histogramme 50 buckets auto calculé dans la même requête que la page ; clic = chip de plage ;
  chips désactivables (barrés) sans être perdus ; « contexte ±5 min » qui sauvegarde/restaure l'état.
- **Pourquoi** : manipulation directe du temps sans formulaire, réversibilité (essayer une plage, revenir).
- **Comment** : sous la frise PNEX, une bande de densité par acteur alimentée par une requête O2
  `histogram(_timestamp, '<auto>')` bornée à 50-200 buckets sur la fenêtre visible, renvoyée avec les
  événements (un seul aller-retour). État de filtre = `Vec<TimeChip{range, active}>` dans un Signal,
  sérialisé dans l'URL ; « contexte » = pile `Vec<ViewState>` (push/pop, Échap = pop). Rendre Δ
  **réglable** (±1/5/15/60 min) — TS l'a laissé en TODO.

## 7. Trois pièges à éviter

### X1 — Toggle au lieu d'intentions explicites (étoile/fait qui s'inversent)
- **Problème** : poser `__ts_star`, `__ts_hidden` ou `__ts_fact` **bascule** toujours (`event.py:1102-1110`) :
  un double envoi, deux analystes simultanés ou un `add_star()` d'analyseur sur un événement déjà étoilé
  **retire** l'étoile. Le nœud d'historique reçoit le label même quand on vient de l'enlever. En plus,
  `set_label` construit `params` avec la clé **booléenne** `remove: remove` au lieu de `"remove"`
  (`opensearch.py:1678`) → la branche `params.remove` du script painless n'est jamais vraie. L'étoile est
  **par sketch, pas par utilisateur** (removeIf sur `name`+`sketch_id`, `opensearch.py:87`, `:99`).
- **Preuve** : issue ouverte https://github.com/google/timesketch/issues/3434 (« Incorrect label toggling and
  search history updates »), `# TODO(#3434): Fix the label logic.` (`event.py:1087`).
- **Parade PNEX** : API idempotente `PUT /annotations/{ref}/star` / `DELETE …` (état cible, pas toggle),
  contrainte unique `(case, ref, kind, author)` ; l'UI optimiste envoie l'état voulu.

### X2 — Annotations sans portée ni auteur, écrites dans la donnée brute
- **Problème** : tags et attributs d'événement (humains et analyseurs) sont écrits dans le document
  OpenSearch **sans sketch, sans auteur, sans date** (`_tag_event` `event.py:59-97`,
  `interface.Event.add_tags` `interface.py:179-207`). Un SearchIndex étant partageable entre sketches, un
  tag posé dans un dossier apparaît dans les autres ; impossible de dire qui l'a posé ou d'annuler un run
  d'analyseur. L'import **jette silencieusement** les lignes à date illisible (`utils.py:430-440`) et
  suppose UTC pour les dates naïves.
- **Preuve** : code cité ; fuseaux : issues ouvertes https://github.com/google/timesketch/issues/2943
  (« Timezone bug in timefilter input »), https://github.com/google/timesketch/issues/674.
- **Parade PNEX** : jamais d'écriture dans les traces O2 ; toute annotation = ligne Postgres avec
  `case_id`, `author`/`detector`, `created_at`, journalisée dans O2 (journal des changements existant).
  Import CSV/GPX : rapport de rejets visible (ligne, raison) et fuseau explicite demandé si naïf.

### X3 — Récit et rapport en blob JSON écrasé en entier, verrou côté client
- **Problème** : Story (et le rapport d'investigation) = un `content` JSON remplacé à chaque sauvegarde
  (`story.py:237-239`) ; le store v3 fait un read-modify-write local puis PUT complet
  (`stores/app.js:79-95`) → **dernière écriture gagne** entre analystes. Le « verrou » `VERIFIED` n'est
  qu'un champ de ce JSON lu par l'UI (`app.js:341-343`) ; le serveur n'en sait rien et accepte toute
  écriture. Pas d'historique. Les blocs « saved search » d'une story sont **des requêtes vivantes**
  (`Story.vue:468-495`) : le rapport change si les données changent.
- **Perf/infra** (même famille « tout dans un gros objet ») : `/api/v1/sketches/<id>` lent avec beaucoup de
  datasources (https://github.com/google/timesketch/issues/3075, ouverte) ; OpenSearch obligatoire et
  fragile en mono-nœud (shards non assignés → 500, https://github.com/google/timesketch/issues/2353) ;
  endpoint de champs inefficace (https://github.com/google/timesketch/issues/3200).
- **Parade PNEX** : récit = blocs en lignes (`case_blocks(id, case_id, ord, kind, payload, version)`)
  avec concurrence optimiste (`expected_version`, 409) — déjà notre pratique ; gel = snapshot JSON
  matérialisé (événements + attachements **résolus**) + SHA-256, vérifié côté serveur (refus d'écriture
  si gelé). Pas de dépendance à un moteur de recherche lourd : Postgres + O2 suffisent au Pi 5.

## 8. Réponses aux questions ouvertes du PRD

**(a) Traces importées CSV/GPX vs flux.** TS ne connaît que l'import fichier → index (copie), avec
mapping des colonnes obligatoires et une timeline « Manual events » pour les saisies. Leçons : (1) un
**schéma minimal imposé** (instant + description + type de temps) rend tout source hétérogène affichable
sur une frise commune — PNEX : `ts`, `lon/lat` optionnels, `label`, `time_kind` ; (2) le **mapping
d'en-têtes dans l'UI** évite de rejeter un fichier ; (3) **ne pas copier** les flux déjà dans O2 : TS n'a
pas le choix car OpenSearch est son seul magasin, PNEX l'a. Recommandation : CSV/GPX importés deviennent un
stream O2 dédié (type « trace importée », `DataSource`-like : nom de fichier, taille, rejets), puis lus
exactement comme un flux live → un seul chemin de lecture kepler.

**(b) Graphe natif vs Cytoscape.js en wasm.** TS utilise Cytoscape.js (`Graph.graph_elements` JSON
Cytoscape, `sketch.py:506-517` ; blocs `TsCytoscapePlugin` dans les stories, `Story.vue:158`), graphes
générés côté serveur par plugins et **mis en cache** (`GraphCache`, `sketch.py:520`). Le graphe y reste
secondaire (onglet + bloc de story) : le cœur du travail est liste + temps. Argument pour PNEX : un
graphe peu central ne justifie pas un moteur natif ; Cytoscape.js via interop JS (comme maplibre) avec
éléments calculés côté Rust (petgraph) et mis en cache par fenêtre temporelle est le chemin le moins
coûteux. TS ne fait **pas** d'estompage temporel ni de rollback d'expansion.

**(c) Nommage.** TS a choisi un mot neutre « **Sketch** » pour le conteneur et réserve « **Investigation** »
au module questions → conclusions → rapport vérifié (`frontend-v3/src/components/Investigation/`),
« Story » au récit. La séparation conteneur / démarche est utile : PNEX peut garder **« Enquête »** comme
nom court de l'objet (le dossier *est* la démarche chez nous, il n'y a pas de module séparé) et éviter
« Projet » (trop générique, collision avec des concepts d'org). « Dossier d'enquête » reste bien pour les
titres longs. Le cas DFIQ montre aussi qu'« hypothèse » mérite d'être un rôle de premier rang (question
ouverte → conclusion → faits).

## 9. Sources

Fichiers lus (racine `bench/timesketch`) :
- `LICENSE`, `docs/guides/user/import-from-json-csv.md`, `docs/guides/user/sketch-overview.md`, `docs/assets/images/` (listing)
- `timesketch/app.py:62-76`
- `timesketch/models/sketch.py` (intégral, structure), `timesketch/models/annotations.py`, `timesketch/models/__init__.py:78-83`
- `timesketch/api/v1/resources/event.py` (59-470, 564-600, 785-1360), `explore.py:200-380`, `story.py:69-245`
- `timesketch/lib/datastores/opensearch.py:78-104, 1641-1725`
- `timesketch/lib/utils.py:46, 130-152, 400-470`
- `timesketch/lib/analyzers/interface.py` (76-320, 725-800, 1211-1300), `chain.py`, `sessionizer.py`, `similarity_scorer.py`, `geoip.py` (grep), `llm_log_analyzer.py` (grep), `dfiq_plugins/manager.py` (grep)
- `timesketch/lib/similarity.py:108-125`, `timesketch/lib/tasks.py:535-551`, `timesketch/lib/llms/features/log_analyzer.py:490-575`, `llm_starred_events_report.py` (grep)
- `tests/test_data/dfiq/questions/Q1001.yaml`
- frontend-ng : `views/Explore.vue` (160-250, 540-700), `views/Story.vue` (114-135, 466-600), `components/Explore/EventList.vue` (100-110, 190, 370-500, 1166-1177), `BarChart.vue`, `EventTagMenu.vue`, `EventActionMenu.vue`, `EventTags.vue`, `Comments.vue`
- frontend-v3 : `stores/app.js`, `components/Explore/EventList.vue` (1-200, 441, 886-890, 1255-1280), `components/Investigation/EventsLog/index.vue`, `ConclusionFact.vue`, `ReportView/QuestionActionsStrip.vue`, `ReportView/index.vue` (grep 320-420), `ReportView/SummarySection.vue`, `Sidebar/FilterBar.vue`

URLs :
- https://github.com/google/timesketch
- https://timesketch.org/guides/user/sketch-overview/
- https://github.com/google/timesketch/issues/3434
- https://github.com/google/timesketch/issues/3075
- https://github.com/google/timesketch/issues/3200
- https://github.com/google/timesketch/issues/2353
- https://github.com/google/timesketch/issues/2943
- https://github.com/google/timesketch/issues/674
