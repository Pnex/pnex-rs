# Fiche benchmark : DFIR-IRIS (iris-web v3.0.0-beta.1) + fork IRIS-NG

> Chemins relatifs à `bench/iris-web`.
> `BE/` = `iris-backend/source/app/`, `FE/` = `iris-frontend/src/routes/(app)/case/[case_id]/`.

## 1. Identité

- **Dépôt parapluie** : `github.com/dfir-iris/iris-web`, commit `37c5521` (merge PR #1139, version
  « v3.0.0-beta.1 » d'après `README.md`). Depuis la v3, le code vit dans **deux sous-modules** (`.gitmodules`),
  vides dans le clone ; je les ai initialisés en `--depth 1` (aucun build) :
  - `iris-backend` @ `f94eb25` (Python/Flask, SQLAlchemy, Alembic, Celery) ;
  - `iris-frontend` @ `ddff6a8` (SvelteKit 5 SSR + Tailwind, `vis-network`, `vis-timeline`, `echarts` : `iris-frontend/package.json:90,107-109`).
- **Licences (lues)** :
  - `LICENSE.txt` du parapluie et `iris-backend/LICENSE.txt` : **LGPL-3.0**. En-têtes Python :
    « GNU Lesser General Public License … version 3 … or any later » (`BE/models/cases.py:1-17`, `BE/business/events.py:1-17`),
    copyright Airbus CyberSecurity (2021) puis DFIR-IRIS (2025).
  - **`iris-frontend/LICENSE.txt` : GNU AGPL-3.0**, et `package.json:4` `"license": "AGPL-3.0-or-later"`.
    Le nouveau front v3 est donc sous une licence **plus contagieuse** que le backend (copyleft réseau). Les fichiers
    `.svelte` n'ont pas d'en-tête : c'est le LICENSE du sous-module qui fait foi.
  - Aucun sous-dossier « enterprise » ; `GPL-3.0.txt` est fourni car la LGPL s'y réfère.
- **Ce que ça permet à PNEX (MIT)** : **inspiration seule, aucune reprise de code**, ni Python (LGPL : une reprise
  imposerait LGPL sur la partie dérivée) ni Svelte (AGPL : incompatible avec une distribution MIT). On reprend des
  idées de modèle et d'interaction, réécrites from scratch en Rust/Dioxus. Pas de copie de CSS, d'icônes
  (`/static/assets/img/graph/*`) ni de textes.
- **Fork IRIS-NG** : `github.com/zach115th/iris-ng` (fork déclaré de `dfir-iris/iris-web`, 3 étoiles, dernier push
  2026-10-10, **LGPL-3.0**, README « Modifications must remain LGPL »). Même verdict : inspiration seule.

## 2. Ce que fait le projet

Plateforme collaborative de réponse à incident : un **case** regroupe assets (machines, comptes), IOCs, evidences
(fichiers reçus), notes Markdown, tâches et une **timeline d'événements** datés liés aux assets/IOCs ; un graphe
asset↔IOC est dérivé de la timeline ; les alertes SIEM peuvent être escaladées en case. La v3 ajoute des **timelines
nommées multiples par case** et des **war rooms** multi-cases avec timeline polymorphe.
**Ce qui nous concerne** : l'événement daté comme pivot, les tables d'attache événement↔objet, le traitement des
fuseaux, le graphe dérivé des co-occurrences, l'import CSV, et (IRIS-NG) la séparation timeline brute / timeline de travail.

## 3. Modèle de données pertinent

### 3.1 Case
`BE/models/cases.py:46-85` `Cases` : `case_id`, `case_uuid`, `name`, `description`, `open_date`/`close_date` (**Date**),
`initial_date`, `client_id` (org cliente), `owner_id`, `reviewer_id`, `state_id`, `severity_id`, `classification_id`,
`custom_attributes` (JSON), `modification_history` (JSON). Tags via `case_tags` (`:117-124`), alertes via
`alert_case_association` (`:79`). Protagonistes (`CaseProtagonist`, `:229-240`) : `name`, `contact`, `role` **texte
libre** — le seul « rôle » du modèle, et il est attaché au case, pas à un événement.

### 3.2 Événement (`cases_events`)
`BE/models/cases.py:127-162` `CasesEvent` :
- identité : `event_id` (BigInt), `event_uuid` (`:132`), `case_id` ;
- contenu : `event_title`, `event_content` (description), `event_source` (texte libre, ex. « Suricata »),
  `event_raw` (payload brut conservé pour la preuve), `event_tags` (**CSV dans un Text**, `:146`) ;
- **temps** : `event_date` (DateTime **sans tz**, `:139`), `event_tz` (Text, ex. `+02:00`, `:147`),
  `event_date_wtz` (DateTime, `:148`), `event_added` (date de saisie, `:140`). **Instant uniquement** : pas de fin,
  pas de durée, pas d'incertitude ;
- **drapeaux de projection** : `event_in_graph` (`:141`), `event_in_summary` (`:142`, utilisé par le rapport
  `BE/datamgmt/reporter/report_db.py:206-224`), `event_is_flagged` (triage, `:149`), `event_color` (`:145`) ;
- **hiérarchie** : `parent_event_id` auto-référence (`:131`, `:158`), contrainte `event_id != parent_event_id` (`:160-162`) ;
  aucune protection contre les cycles longs ;
- catégorie : `case_events_category` (`BE/models/models.py:145-153`) avec `event_id` **unique** → une seule catégorie
  par événement, tirée d'une liste globale `event_category` (`:138-142`, catégories MITRE-like « Initial access »,
  « Malware »… visibles sur `img/demo_timeline.png`) ;
- provenance/versionnage : `user_id` (auteur), `modification_history` JSONB écrit par `add_obj_history_entry`
  (`BE/util.py:49-74`) = `{timestamp: {user, user_id, action}}` → **qui/quand/quoi, sans diff ni valeur antérieure**.

**Sémantique des trois champs de date** (`BE/schema/marshables.py:1259-1285`, `validate_date`) :
```
event_date     = isoparse(date_str + tz_str)   # datetime « aware »
event_date_wtz = isoparse(date_str)            # heure murale, sans offset
```
Les deux colonnes étant `timestamp without time zone`, `event_date` finit stocké normalisé (UTC via le driver) et
`event_date_wtz` garde l'heure locale saisie ; `event_tz` garde l'offset texte. L'export CSV du front libelle bien
`event_date` « Date (UTC) » et `event_date_wtz` « Date (with TZ) » (`FE/timeline/+page.svelte:653-658`). C'est un
**offset fixe**, jamais un nom IANA : un événement ne sait pas qu'il était « Europe/Paris en heure d'été ».

### 3.3 Liens événement ↔ objets (tables d'association)
- `case_events_assets` (`BE/models/models.py:98-108`) : `id`, `event_id`, `asset_id`, `case_id` ;
- `case_events_ioc` (`:111-121`) : `id`, `event_id`, `ioc_id`, `case_id` ;
- **aucun lien événement ↔ evidence, ↔ note ou ↔ tâche** : vérifié par grep, absent du backend amont
  (c'est d'ailleurs un ajout d'IRIS-NG, cf. §3.7, et une FR #640 « screenshots on timeline events ») ;
- **aucune colonne de rôle** sur ces jointures : l'asset est « lié », point. Pas de `valid_from/valid_to`, pas d'auteur
  du lien. Le `case_id` est dénormalisé dans la jointure pour filtrer vite par case ;
- réécriture **delete-all puis insert** à chaque sauvegarde (`BE/datamgmt/case/case_events_db.py:252-295`), avec option
  `sync_iocs_assets` qui crée aussi des liens `ioc_asset_link` (asset×IOC) pour chaque paire de l'événement (`:276-289`).

### 3.4 Timelines nommées (v3)
`BE/models/cases.py:165-216` :
- `case_timelines` : `timeline_id`, `case_id`, `name` (unique par case), `description`, `color` (#RRGGBB),
  `is_default` (« Main » créée à la création du case), `created_by_id` ;
- `case_event_timelines` : jointure pure (event, timeline), « per-(event, timeline) metadata is intentionally out of
  scope » (docstring `:200-207`). Un événement peut figurer sur « Attacker activity » ET « Network » sans duplication ;
  sans `timeline_ids`, il est rattaché à la timeline par défaut (`BE/business/events.py:76-81`).

### 3.5 War room : timeline polymorphe multi-cases (v3)
`BE/models/war_rooms.py:203-310` `WarRoomTimelineEvent` : **soit** une référence `(case_id, event_id)` vers un
événement de case, **soit** une entrée libre (`title`, `event_date`, `event_tz`) — contrainte CHECK « les deux nuls ou
les deux renseignés » (`:248-253`). `ON DELETE SET NULL` sur `parent_id` pour ne pas perdre les enfants (`:268-273`),
`CASCADE` sur la référence d'événement. Assets/IOCs épinglés par M2M depuis **n'importe quel case** (`:312-361`).
C'est le patron « timeline de travail qui référence la source sans la copier ».

### 3.6 Evidences, notes, tâches, templates
- `case_received_file` (`BE/models/evidences.py:47-65`) : `filename`, `file_hash` (Text, algo non typé), `file_size`
  (BigInt), `acquisition_date`, **`start_date`/`end_date`** (période couverte par la preuve — le seul intervalle du
  modèle), `chain_of_custody` (JSON libre), `type_id`, `modification_history`.
- Notes (`BE/models/models.py:301-334`) : `notes` + **`note_revisions`** (`revision_number`, titre, contenu, auteur,
  horodatage) — vrai versionnage, contrairement aux événements. Arborescence `note_directory` (`:337-346`).
- Tâches `case_tasks` (`:396-419`) : statut, ouverture/fermeture, assignés (`task_assignee`), pas de lien vers événement.
- Templates `case_template` (`:51-78`) : `title_prefix`, `summary`, `tags`, `tasks`, `note_directories` **en JSON**,
  appliqués à la création (`BE/datamgmt/manage/manage_case_templates_db.py:161-230` : préfixe du nom, classification,
  tâches et notes instanciées via les mêmes schémas de validation + hooks de modules).

### 3.7 IRIS-NG : timeline brute vs timeline de travail
Migration `source/app/alembic/versions/b9e1f4a72c83_add_case_working_event.py` (lue via l'API GitHub) :
table **`case_working_event`** à côté de `cases_events` : `source` (outil), `event_date`, `event_title`,
`event_description`, `event_source_host`, `severity`, `mitre_techniques`, `external_id`, `event_raw` **JSONB**,
`import_batch_id` (UUID), **`status` ∈ {pending, true_positive, false_positive}** (CHECK), `promoted_event_id` →
`cases_events` (`SET NULL`), `reviewed_at`/`reviewed_by`, index `(case_id, event_date)`.
Docstring : « Tool-ingested timeline rows … land here as `pending` before an analyst reviews them. Promote → spawns
a real `cases_events` row … Reject → `false_positive` ».

## 4. Patterns UX observés

**Captures** : dépôt `img/demo_timeline.png` (timeline v1 : badge de date à gauche, cartes colorées, valeurs d'IOC
surlignées en orange dans le texte, assets liés en liens bleus, catégorie en pastille, boutons Visualize/Refresh/Add
event, filtre « All assets ») ; `img/image.png` (accueil). Docs : `https://docs.dfir-iris.org/operations/cases/case_operations/`
(captures `https://dfir-iris.org/_static/cases/*.png`, case uniquement) ; GIF `docs/_static/timeline_speed.gif` du
dépôt `dfir-iris/iris-doc-src`. Démo publique : `https://preview.dfir-iris.org` (README). Aucune page de doc dédiée à
la timeline n'existe (404 sur les URL candidates, arbre du dépôt doc sans `timeline.md`).

### 4.1 Timeline éditable v3 (liste verticale, pas une frise)
- **Ce que voit l'utilisateur** : cartes groupées par jour (`FE/timeline/+page.svelte:246-260`, groupage par
  `toLocaleDateString`), en-tête « Today · Oct 12 », rail vertical coloré (couleur de l'événement sinon de la
  catégorie, `FE/timeline/components/timeline-event-card.svelte:102-126`), heure locale (`:82-91`), source, flag,
  tags `#x`, chips assets/IOCs cliquables (`:375-415`).
- **Deux modes** `list | tree` (`FE/timeline/components/timeline-view.svelte:17,76-87`) : en arbre, les racines
  alternent gauche/droite d'une épine centrale avec un index stable sur toute la timeline ; les enfants sont indentés
  avec des coudes en L (`:124-183`). Plier/déplier par nœud et global (`+page.svelte:315-340`).
- **Sélection multiple** (`toggleSelecting`, `+page.svelte:297-313`) pour actions groupées.
- **Recherche rapide** in-memory avec navigation précédent/suivant entre correspondances et surlignage
  (`+page.svelte:141-210`, `components/highlight.svelte`) ; au premier usage elle **charge toutes les pages**
  (`loadAll`, `:204-210`) pour couvrir la timeline entière.
- **Filtres serveur** (`components/timeline-filters.svelte`) : titres, sources, descriptions, raw, tags, catégories,
  assets, IOCs, dates, flag → `BE/datamgmt/case/case_events_db.py:429-566` (ILIKE cumulés en AND).
- **Barre latérale des timelines nommées** (`components/timeline-sidebar.svelte:1-260`) : cases à cocher multi, pastille
  couleur, création/édition inline ; sélection vide = tout ; filtrage **côté client** sur `timeline_ids` estampillés
  sur chaque ligne (`+page.svelte:100-140`).
- **Lien partageable vers un événement** : copie URL ou lien Markdown `[#id titre](url)` (`timeline-event-card.svelte:215-222`),
  l'événement partagé est mis en évidence à l'ouverture (`:93,260`).
- **Saisie d'événement** (`components/timeline-event-form.svelte:150-195`) : champ Date (`type=date`), champ Time texte
  `00:00:00.000`, champ **TZ texte libre `+00:00`** ; parent via sélecteur (`:224-228`), timelines (`:376-380`),
  interrupteurs « in summary »/« in graph » (`:407-416`), palette de couleurs (`:123,431-446`). Côté v1, un endpoint
  « convert-date » devine la date collée (`BE/blueprints/rest/case/case_timeline_routes.py:619-640`).
- **Défilement infini** avec déclencheur un écran en avance (`+page.svelte:107-126,780-830`) ; le serveur
  **greffe la lignée** (ancêtres et descendants) de chaque événement de la page (`BE/business/events.py:244-299`).

### 4.2 Timeline visuelle (vis-timeline) : abandonnée en v3
v1 : page `/case/timeline/visualize` (`BE/blueprints/pages/case/case_timeline_routes.py:61-68`) alimentée par
`/visualize/data/by-asset` (une ligne = un asset, item = événement) et `/by-category` (une ligne = catégorie)
(`BE/blueprints/rest/case/case_timeline_routes.py:200-258`). En v3, `iris-frontend/src/lib/components/common/VisTimeline.svelte`
existe (wrapper `DataSet` + `Timeline`, `fit()` à chaque mise à jour) mais **n'est importé nulle part** : la frise
par asset a disparu de l'UI v3. Issues ouvertes : #233 (filtrer la frise par IOC/tag), #993 (frise par IOC & tag),
#100 (export PNG sans graduations).

### 4.3 Graphe de case (vis-network)
`FE/graph/+page.svelte` : nœuds **assets et IOCs seulement** (les événements ne sont pas des nœuds), arêtes =
co-occurrence dans un même événement `event_in_graph` ; barre d'outils recherche + chips de groupes + zoom + fit +
« Freeze layout/Resume physics » (`:543-640`) ; physique barnesHut puis **gel automatique** à la fin de la stabilisation
(`:488-505`) ; la sélection est poussée dans vis-network **sans reconstruire le dataset** pour ne pas relancer la
mise en page (`:391-405`) ; rail de détails avec voisins (`:373-390`), menu contextuel (`:461-480`), estompage des
nœuds hors recherche (`:275-330`). `layout.randomSeed = caseId` (`:98`) → même disposition à chaque ouverture.

### 4.4 Import CSV
- v1 serveur (`BE/blueprints/rest/case/case_timeline_routes.py:646-800`) : colonnes imposées `event_date, event_tz,
  event_title, event_category, event_content, event_raw, event_source, event_assets (;), event_iocs (|), event_tags (|)` ;
  **validation complète avant écriture** (asset/IOC/catégorie inconnus → erreur avec numéro de ligne), puis insertion
  dans `begin_nested()` (`:761`). Options globales : `event_in_summary`, `event_in_graph`, `event_source`, `sync_iocs_assets`.
- v3 : le commentaire `:642-645` annonce « Not ported on purpose » ; le front parse le CSV et fait **un POST par ligne**
  (`FE/timeline/+page.svelte:715-776`), sans assets ni IOCs, catégorie par défaut.

### 4.5 Alerte → case
`BE/datamgmt/alerts/alerts_db.py:752-880` et `:888-1060` : à l'escalade, l'analyste choisit les IOCs/assets de l'alerte
à importer (copie profonde si déjà rattachés ailleurs, `:972-1006`), un template (préfixe de titre), et
`import_as_event` → un événement `[ALERT] titre` avec `event_raw = json(alert_source_content)`, `event_source =
alert_source`, date = `alert_source_event_time`, `event_tz='+00:00'`, liens assets/IOCs (`:847-876`, `:1018-1035`).
Fusion d'alertes dans un case existant : `merge_alert_in_case` (`:1061`), et `unmerge` (`:1197`).

## 5. Algorithmes / mécanismes réutilisables

### 5.1 Graphe de co-occurrence dérivé des événements
Version v1 serveur (`BE/blueprints/rest/case/case_graphs_routes.py:41-140`) :
```
rows = jointures (event in_graph) × assets  ∪  (event in_graph) × iocs
pour chaque row : nœud 'a'+asset_id ou 'b'+ioc_id (dédup)   ; regroupe par event_id
pour chaque event : pour chaque paire (x,y) de ses objets (itertools.combinations) :
    si x,y tous deux IOC et l'événement a > 2 objets : ignorer
    arête x–y, title = "date - titre événement", dashes si un IOC
```
Version v3 client (`FE/graph/+page.svelte:158-241`) : même boucle mais **uniquement arêtes asset↔IOC**
(`fromIsAsset === toIsAsset → continue`), dédup par clé triée `from|to` (le **premier** événement donne le titre).
Complexité O(Σ k²) avec k = objets par événement ; la dédup v1 `any(node['id']==idx …)` (`:97`) est O(n²).
Cas limites : `date = '15-05-2021'` en repli si la date plante (`:79`) ; `if date not in dates` teste les **clés** du
dict (`:81`) donc toujours vrai ; v3 résout les assets **par nom** (`assetByName`, `:160-176`), un renommage casse le lien.

### 5.2 Parseur de date « brute force »
`BE/iris_engine/utils/common.py:58-100` : 10 chiffres → epoch s, 13 → epoch ms, sinon essai séquentiel d'une
soixantaine de formats `strptime` (ISO, `%d/%m/%Y`, syslog `%b %d %H:%M:%S`, RFC 2822…). Premier qui passe gagne.
Pièges : `datetime.fromtimestamp` = **fuseau du serveur** ; `%Y-%d-%m` après `%Y-%m-%d` → ambiguïté jour/mois
silencieuse ; formats syslog sans année → 1900.

### 5.3 Pagination qui préserve la lignée
`BE/business/events.py:244-299` : tranche la liste triée par date, puis BFS descendants (via `children_by_parent`) et
remontée des ancêtres pour que chaque page soit un **sous-arbre fermé** (sinon le SPA promeut en racines les enfants
orphelins). Coût : tout est chargé puis filtré en Python (`get_filtered_case_events` sans LIMIT, `:429-566`).

### 5.4 IRIS-NG : collapse de fan-out à l'import
`source/app/iris_engine/working_timeline/hayabusa_parser.py` : Hayabusa émet une ligne par règle Sigma ; groupage sur
la clé naturelle `(Timestamp, Computer, Channel, EventID, RecordID)` (`_group_key`), titre = règle de plus haute
sévérité, description = toutes les règles. Import plafonné à 25 k lignes (README), filtre de dates optionnel côté
serveur, `import_batch_id` pour supprimer un lot entier (`DELETE …/imports/<batch_id>`). Promotion
(`blueprints/rest/v2/cases/working_timeline.py:596-700`) : verrou de ligne + idempotence (`status == true_positive` →
renvoie l'existant), création du `CasesEvent` **flaggé « à revoir »**, historique « promoted from working timeline
(source) », matérialisation des assets/IOCs **seulement à la promotion, jamais à l'import**.

## 6. Trois patterns à reprendre dans PNEX

### P1. Timeline brute (propositions) séparée de la timeline de travail, avec promotion explicite
- **Quoi** : IRIS-NG `case_working_event` (pending/true_positive/false_positive, `promoted_event_id`, `import_batch_id`,
  `reviewed_by`). C'est exactement la règle PRD « détecteurs toujours PROPOSÉS, jamais ajoutés d'office ».
- **Pourquoi** : un import CSV/GPS ou un détecteur produit des centaines de lignes ; mélangées aux événements
  curés, elles noient le récit (IRIS-NG a dû ajouter un « duplicate detection » a posteriori).
- **Comment** : table PG `case_proposals (id, case_id, kind enum('detector','import'), detector text, params jsonb,
  t_start timestamptz, t_end timestamptz null, payload jsonb, batch_id uuid, status enum('pending','accepted',
  'rejected'), event_id uuid null → objects, reviewed_by, reviewed_at)`. Détecteurs Rust (co-présence, geofence,
  arrêts, fin de signal) écrivent ici ; « Accepter » crée l'objet événement + ses `resource_edges` dans **une
  transaction**, idempotent (`SELECT … FOR UPDATE`, retour de l'existant si déjà accepté) ; journal O2
  `case.proposal.accepted` avec la provenance (détecteur + paramètres). UI Dioxus : bac « Propositions » avec
  compteurs « 23 en attente · 4 acceptées · 2 rejetées » et rejet par lot (`batch_id`).

### P2. Événement = pivot, attaches via jointure dédiée + appartenances multiples à des « vues »
- **Quoi** : `case_events_assets`/`case_events_ioc` + `case_event_timelines` (un événement sur N timelines nommées
  colorées sans duplication) + war room « référence OU entrée libre ».
- **Pourquoi** : correspond au modèle PRD (on attache des objets à l'événement) et aux « lignes par acteur » ; les
  timelines nommées d'IRIS ≈ nos filtres « suivi d'un acteur » persistés.
- **Comment** : pas de tables par type comme IRIS (une par asset/IOC) — une seule arête générique
  `resource_edges(src=event, dst=objet, kind='attached', role enum('evidence','actor','context','hypothesis','source'),
  valid_from, valid_to, created_by)` ; le **rôle sur l'arête** comble le manque d'IRIS (#1150 réclame justement des
  hypothèses avec preuves pour/contre). Les « vues » = tag d'arête ou table `case_lanes(event_id, lane_id)` ;
  membres du dossier = `SELECT DISTINCT dst FROM resource_edges WHERE src IN (événements du case)`.

### P3. Graphe dérivé, figé après stabilisation, sélection sans reconstruction
- **Quoi** : graphe calculé depuis les attaches (pas saisi), drapeau `in_graph` par événement, `randomSeed = caseId`,
  gel de la physique en fin de stabilisation, mise à jour de sélection sans recréer le dataset (`FE/graph/+page.svelte:96-153,391-405,488-505`).
- **Pourquoi** : stabilité visuelle (même carte mentale à chaque ouverture), pas de « danse » au clic.
- **Comment** : en Rust, calcul des positions côté serveur ou wasm (force simple + seed déterministe = hash de
  l'id du dossier), positions mémorisées par nœud dans le snapshot du dossier ; rendu SVG/Canvas Dioxus. Mieux qu'IRIS :
  les **événements sont des nœuds** (biparti objet–événement, rôle = style d'arête) au lieu d'une clique entre objets,
  ce qui évite l'explosion O(k²) et garde le « quand » ; l'estompage temporel = opacité selon `|t_event − curseur|`.

## 7. Trois pièges à éviter

### T1. Fuseaux horaires : offset texte, dates naïves, affichage local d'un « UTC »
- **Problème** : `event_tz` est un offset fixe saisi en texte libre (`timeline-event-form.svelte:186-190`) ; colonnes
  `timestamp without time zone` ; le front fait `new Date("2024-01-01T10:00:00.000000")` **sans `Z`**
  (`timeline-event-card.svelte:82`, groupage `+page.svelte:252`) → interprété en heure du navigateur alors que
  l'export dit « Date (UTC) » ; `fromtimestamp` en fuseau serveur (`common.py:63`) ; alerte sans date → `datetime.now()`
  local (`alerts_db.py:1027`) ; IRIS-NG **jette l'offset** Hayabusa et suppose UTC (`hayabusa_parser.py:_parse_timestamp`).
- **Preuves** : #722 (badge de date décalé d'un jour), #730 (« timestamps recorded in UTC-8 », aucun réglage),
  #1054 (heure de création d'alerte décalée de 3 h), #997 (offset absent de l'API pour les rapports).
- **Parade PNEX** : `timestamptz` partout, sérialisation RFC 3339 avec `Z` ; conserver en plus le **nom IANA** source
  (`tz_name`) et l'offset original pour l'affichage « heure locale du lieu » ; à l'import CSV, fuseau **obligatoire par
  lot** (choisi dans l'aperçu), jamais deviné ; affichage frise : bascule explicite UTC / local navigateur / local du
  lieu, toujours étiquetée.

### T2. Import CSV non aller-retour, non atomique et parseur fragile
- **Problème** : export « Date (UTC), Title, Description… » vs import `event_date(UTC), event_title…`
  (`+page.svelte:653-665` vs `:750-756`) ; regex `([^,]+)` qui **saute les cellules vides** → décalage de colonnes
  (`:715-719`) ; split par ligne qui casse les champs multilignes (`:735-739`) ; un POST par ligne → import partiel en
  cas d'échec, aucun assets/IOC ; descriptions : `\n` remplacés par « - » à l'export et inversement à l'import (perte).
- **Preuves** : #639 (« cannot import the exported csv »), #976 (virgules non échappées), #697 (pas de suppression en
  masse / doublons), commentaire « Not ported on purpose » (`case_timeline_routes.py:642-645`).
- **Parade PNEX** : un seul schéma d'échange versionné (JSON + CSV aux **mêmes en-têtes**), test de round-trip ;
  parseur `csv` crate côté serveur, aperçu + mapping de colonnes, validation complète puis **une transaction** ;
  lot identifié (`batch_id`) atterrissant en propositions (P1) pour annulation d'un clic ; traces GPS jamais copiées.

### T3. Tout charger puis filtrer en mémoire ; graphe en clique ; hiérarchie fragile
- **Problème** : `get_filtered_case_events` sans LIMIT puis filtrage assets/IOCs et pagination en Python
  (`BE/business/events.py:131-299`) ; la recherche rapide v3 force `loadAll` ; graphe v1 O(n²) + cliques ; parenté
  sans garde de cycle ni de profondeur, rendu des petits-enfants cassé en v1 (#594) ; réattacher un parent impose un
  menu déroulant de titres (#992 demande le glisser-déposer) ; FE v3 résout les assets **par nom**.
- **Parade PNEX** : fenêtre temporelle côté SQL (`WHERE t && tstzrange($1,$2)` + index GiST sur un `tstzrange`),
  agrégats par acteur pour la frise, détail chargé à la demande ; références par **id** d'objet uniquement ; pas de
  parenté libre entre événements : préférer « période englobante » (un événement période contient des instants par
  inclusion temporelle) — si parenté il y a, CHECK anti-cycle (CTE récursive) et glisser-déposer.

## 8. Réponses aux questions ouvertes du PRD

- **(a) Traces importées CSV/GPX vs flux** : IRIS ne gère que des événements discrets ; son import CSV crée des
  événements, ce qui pour des milliers de points GPS serait catastrophique (pas de pagination SQL, cliques de graphe).
  IRIS-NG montre la bonne voie : **ingérer en zone de staging**, regrouper (collapse sur clé naturelle), promouvoir à la
  main. Pour PNEX : un CSV/GPX importé devient une **série** (O2, stream dédié par import, `batch_id`, fuseau déclaré)
  traitée exactement comme un flux device, lue à la volée au format kepler ; seuls les **événements** dérivés
  (arrêts, entrées geofence) sont proposés dans le dossier. Même chemin de lecture pour import et flux.
- **(b) Graphe natif vs Cytoscape.js en wasm** : IRIS a gardé une lib JS (vis-network) et en a vécu les limites —
  physique à geler à la main, dataset à ne pas reconstruire, résolution par nom ; la FR #1147 demande une mise en
  page hiérarchique (arbre de processus, phases), #111 un graphe plus interactif ; IRIS-NG est passé à **D3 v7**. À
  l'échelle d'un dossier (dizaines à centaines de nœuds), un rendu **natif SVG Dioxus** avec force déterministe suffit
  et évite le pont JS ; garder Cytoscape en réserve seulement si les mises en page hiérarchiques (dagre/elk) deviennent
  nécessaires.
- **(c) Nommage** : IRIS dit **« Case »** (UI et API `/api/v2/cases`), « War room » pour l'agrégat multi-cases ;
  vocabulaire DFIR/SOC, connoté police/sécurité. Pour nos personas (RCA industrielle, journaliste), « Dossier
  d'enquête » reste le plus juste en français ; « Enquête » est l'action, « Projet » est trop générique. Si un jour on
  agrège plusieurs dossiers, IRIS suggère un nom distinct pour le niveau au-dessus (≈ « Salle de crise »).

## 9. Sources

**Fichiers lus** (iris-web `37c5521`, backend `f94eb25`, frontend `ddff6a8`) :
- `LICENSE.txt`, `README.md`, `.gitmodules`, `docs/upgrade-to-3.0.0.md`, `img/demo_timeline.png`
- `iris-backend/LICENSE.txt`, `iris-frontend/LICENSE.txt`, `iris-frontend/package.json`
- `BE/models/cases.py`, `BE/models/models.py` (l. 51-421), `BE/models/evidences.py`, `BE/models/war_rooms.py` (l. 10-361)
- `BE/business/events.py`, `BE/business/war_room_timelines.py` (en-tête)
- `BE/datamgmt/case/case_events_db.py` (l. 44-97, 252-300, 429-566), `BE/datamgmt/alerts/alerts_db.py` (l. 752-1060),
  `BE/datamgmt/manage/manage_case_templates_db.py` (l. 161-230), `BE/datamgmt/states.py` (l. 82-105),
  `BE/datamgmt/reporter/report_db.py` (l. 206-224)
- `BE/schema/marshables.py` (l. 1232-1340), `BE/util.py` (l. 49-74), `BE/iris_engine/utils/common.py` (l. 58-100)
- `BE/blueprints/rest/case/case_timeline_routes.py` (l. 186-260, 558-800), `BE/blueprints/rest/case/case_graphs_routes.py`,
  `BE/blueprints/pages/case/case_timeline_routes.py` (l. 55-80), `BE/blueprints/rest/v2/case_routes/events.py` (l. 60)
- `FE/timeline/+page.svelte`, `FE/timeline/components/timeline-view.svelte`, `timeline-event-card.svelte`,
  `timeline-event-form.svelte`, `timeline-sidebar.svelte`, `FE/graph/+page.svelte` (l. 1-505),
  `iris-frontend/src/lib/components/common/VisTimeline.svelte`
- IRIS-NG via API GitHub : `README.md`, `source/app/alembic/versions/b9e1f4a72c83_add_case_working_event.py`,
  `source/app/blueprints/rest/v2/cases/working_timeline.py` (l. 1-15, 596-700),
  `source/app/iris_engine/working_timeline/hayabusa_parser.py` (l. 1-20, 232-282)

**URLs** :
- https://github.com/dfir-iris/iris-web · https://github.com/dfir-iris/iris-backend · https://github.com/dfir-iris/iris-frontend
- https://github.com/zach115th/iris-ng (licence LGPL-3.0, fork de dfir-iris/iris-web) ·
  https://github.com/zach115th/iris-ng/releases/tag/IRIS-NG-v2.4.6
- https://docs.dfir-iris.org/latest/ · https://docs.dfir-iris.org/operations/cases/case_operations/ ·
  https://github.com/dfir-iris/iris-doc-src (arbre : `docs/_static/timeline_speed.gif`) · https://preview.dfir-iris.org
- Issues : https://github.com/dfir-iris/iris-web/issues/722 · /730 · /997 · /1054 (fuseaux) · /639 · /976 · /697 (CSV) ·
  /594 · /992 (hiérarchie) · /233 · /993 · /100 (frise visuelle) · /111 · /1147 (graphe) · /1150 (hypothèses) · /640 (pièces sur événement)
