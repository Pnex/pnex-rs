# Fiche benchmark : OpenCTI (investigations, cases, graphe, timeline)

Racine du clone : `bench/opencti`. Chemins ci-dessous relatifs à
`opencti-platform/` sauf mention contraire (`F/` = `opencti-front/src/`, `G/` = `opencti-graphql/src/`).

## 1. Identité

- Dépôt : https://github.com/OpenCTI-Platform/opencti (monorepo : `opencti-front` React/Relay, `opencti-graphql`
  Node/GraphQL sur Elasticsearch/OpenSearch + Redis + RabbitMQ).
- Commit étudié : `5f0be2a` (« chore(deps): update devdependencies (non-major) (#18905) »).
- Licence confirmée en lisant `LICENSE:1-20` (racine) : double régime, **par fichier**.
  - « The OpenCTI Community Edition is licensed under the Apache License, Version 2.0 ».
  - « The OpenCTI Enterprise Edition is licensed under the OpenCTI Enterprise Edition License ».
  - « If no such header is provided, this means that the file is belonging to the Community Edition under the Apache
    License, Version 2.0. » → un fichier sans en-tête = Apache-2.0.
- Fichiers sous **Enterprise Edition License** (grep de l'en-tête dans `G/` et `F/`) :
  - dossier `G/enterprise-edition/` (`ee.ts`, `copyright.md`, `opencti_ca.js`, `xtm_ca.ts`) ;
  - modules back : `G/modules/playbook/**`, `G/modules/pir/**`, `G/modules/disseminationList/**`,
    `G/modules/emailTemplate/**`, `G/modules/ingestion/ingestion-json*`, `G/modules/ai/ai-domain.ts` + `ai-resolver.ts`,
    `G/modules/settings/licensing.ts` + `license-certificate.ts`, `G/modules/xtm/one/xtm-one-license.ts`,
    `G/parser/json-mapper.ts`, `G/domain/indexedFile.ts`, `G/database/file-search.js`, `G/resolvers/indexedFile.js`,
    managers `activityManager.ts`, `activityListener.ts`, `fileIndexManager.ts`, `pirManager.ts`,
    `playbookManager/playbookManager.ts` ;
  - front : `F/private/components/pir/**`, `F/private/components/data/playbooks/**` (dont
    `PlaybookFlowFieldCaseTemplates.tsx`), `F/private/components/common/audits/**`,
    `F/private/components/settings/activity/**`, `F/private/components/settings/file_indexing/**`,
    `F/utils/hooks/useAI.ts`, `F/utils/hooks/useEnterpriseEdition.ts`, `F/private/Analytics.tsx`,
    `F/private/components/LtsLicenseRoot.tsx`, `F/components/dashboard/AuditsWidgetRenderContent.tsx`.
- **Investigations et cases ne dépendent PAS de l'EE** : aucun fichier de `F/private/components/workspaces/**`,
  `F/private/components/cases/**`, `F/components/graph/**`, `G/modules/workspace/**`, `G/modules/case/**` ne porte
  l'en-tête EE ; aucun `checkEnterpriseEdition` dans `G/modules/workspace` ni `G/modules/case`. Seules exceptions : (a)
  les formulaires de création de case importent le hook EE `useEnterpriseEdition` (fichier EE) pour afficher le champ
  `authorized_members` (`F/private/components/cases/case_incidents/CaseIncidentCreation.tsx:27,119,161,354`, idem
  RFI/RFT) ; (b) le résumé IA de container (`G/domain/container.js:283-285`, `checkEnterpriseEdition`) ; (c)
  l'application de case templates par playbook (EE).
- Ce que la licence permet à PNEX (MIT) : le code CE est Apache-2.0, compatible avec une reprise dans un projet MIT
  **avec attribution** (conserver la notice/copyright Filigran, mention des modifications, fichier NOTICE si présent).
  Le code EE : **inspiration seule**, aucune reprise. En pratique la stack (React/Relay/JS) ne se transpose pas en
  Rust/Dioxus : on reprend des **algorithmes et des patterns**, pas du code.

## 2. Ce que fait le projet

Plateforme de Cyber Threat Intelligence : base de connaissance STIX 2.1 (entités SDO/SCO, relations SRO, sightings)
stockée en Elasticsearch, avec containers (Report, Grouping, Cases) et « workspaces » (dashboards, investigations).
Ce qui nous concerne : (1) l'**investigation** = un graphe de pivot persistant, avec expansion filtrée par types et
rollback ; (2) les **Cases** (Incident Response, RFI, RFT) = containers `object_refs` + tâches + assignés ; (3) le
**pont investigation ↔ container** ; (4) le **slider de période** sur le graphe ; (5) la **timeline** de container ; (6)
la modélisation temporelle des relations (`start_time`/`stop_time`, `first_seen`/`last_seen`) avec dates sentinelles.

## 3. Modèle de données pertinent

### 3.1 Workspace « investigation » (pas un objet STIX, un objet interne)
- Déclaration : `G/modules/workspace/workspace.ts:28-36`. Champs : `name`, `description`, `manifest` (config dashboard),
  `type` (`'investigation'` | `'dashboard'`), `tags`, `graph_data` (texte : positions des nœuds, sérialisées base64),
  `investigated_entities_ids` (multiple, **liste d'IDs à plat**, ni rôle ni date), `refresh_interval`.
- Schéma GraphQL : `G/modules/workspace/workspace.graphql:1-37` ; `objects(...)` résout la liste d'IDs en
  entités/relations paginées, `toStixReportBundle` exporte.
- Résolution : `G/modules/workspace/workspace-domain.ts:102-118` → si liste vide pagination vide, sinon filtre
  `internal_id IN investigated_entities_ids` (+ `all: true` = tout charger).
- Validation d'édition : `workspace-domain.ts:120-146` (`checkInvestigatedEntitiesInputs`) : tout ID ajouté/remplacé
  doit exister dans les index data, sinon `FunctionalError('Invalid ids specified')` ; appelée à la ligne 251.
- Droits : `addWorkspace` (`workspace-domain.ts:162-180`) exige la capacité `INVESTIGATION_INUPDATE` ; le créateur est
  ajouté d'office comme admin dans `authorized_members` (`initializeAuthorizedMembers`, ~147-160). Partage identique aux
  dashboards (doc `docs/docs/usage/pivoting.md` « Access restriction »).
- **Les relations ET les nœuds** sont dans la même liste `investigated_entities_ids` : une arête n'apparaît dans
  l'investigation que si son ID y est (cf. `addRelationInGraph`,
  `F/private/components/workspaces/investigations/InvestigationGraph.tsx:506-508`).

### 3.2 Containers et Cases
- Lien d'appartenance = ref relationship `object` (STIX `object_refs`) : `G/schema/stixRefRelationship.ts:736-752` —
  `multiple: true`, cibles = tout objet core, toute relation core/ref, sightings ; `datable: false` → **l'appartenance
  n'est pas datée** et ne porte aucun rôle.
- Case abstrait : `G/modules/case/case.ts:27-35` : `name`, `description`, `content` (texte riche), `content_mapping`,
  `caseTemplate` (ref vers `Case-Template`) ; refs `objectOrganization` (partage).
- Incident Response : `G/modules/case/case-incident/case-incident.ts:38-46` : `severity` (vocab `case_severity_ov`),
  `priority` (`case_priority_ov`), `response_types` (multi, `incident_response_types_ov`), `authorized_members`,
  `authorized_members_activation_date` ; refs `createdBy`, `objectMarking`, `objectAssignee`, `objectParticipant`
  (assignés/participants = personnes au niveau du case, pas au niveau d'un événement).
- RFI : `G/modules/case/case-rfi/case-rfi.ts:39-41` (`information_types`) ; RFT :
  `G/modules/case/case-rft/case-rft.ts:39-41` (`takedown_types`). Tâches et templates :
  `G/modules/case/case-template/*`, `F/private/components/cases/tasks/`.
- Pont container → investigation : `G/modules/workspace/investigation-domain.ts:43-50` (`investigationAddFromContainer`)
  crée un workspace nommé `[Type] "Nom" (horodatage)` avec `container.object` filtré ; mutation
  `containerEdit.investigationAdd` (`G/resolvers/container.js:91`, `@auth KNOWLEDGE_KNUPDATE`).
- Pont investigation → container : `G/domain/container.js:258-266` (`knowledgeAddFromInvestigation`) : prend
  `investigated_entities_ids` sauf le container lui-même, filtre `Task` et `Note` (`filterUnwantedEntitiesOut`,
  `container.js:247-256`), puis crée en masse des refs `object` (`stixCoreObjectAddRelations`). **Ajout pur** : pas de
  copie des positions (`graph_data`), pas de lien persistant investigation↔container.
- Export STIX : `investigation-domain.ts:18-41` : fabrique un Report synthétique (standard_id déterministe `name +
  published=created_at`) contenant tous les objets → bundle STIX.

### 3.3 Temps
- Relation core : `G/modules/attributes/stixCoreRelationship-registrationAttributes.ts:13-14` → `start_time`,
  `stop_time` (date, `mandatoryType: 'customizable'`). Mêmes champs sur les ref relationships
  (`stixRefRelationship-registrationAttributes.ts:10-11`) et relations internes (« First/Last observation »,
  `internalRelationship-registrationAttributes.ts:71-72`).
- Sighting : `stixSightingRelationship-registrationAttributes.ts:10-12` → `attribute_count`, `first_seen`, `last_seen`.
- Entités : `first_seen`/`last_seen` sur Campaign, Intrusion Set, Malware, Threat Actor, Incident
  (`stixDomainObject-registrationAttributes.ts:142-143, 214-215, 223-224, 256…`) ; entité **Event** avec
  `start_time`/`stop_time` (`G/modules/event/event.ts:33-34`).
- **Inconnu = sentinelles**, pas NULL : `G/utils/format.js:63-66` `FROM_START = 0` (1970-01-01) et `UNTIL_END =
  100000000000000` (`5138-11-16T09:46:40.000Z`). À la création : `G/database/data-builder.js:234-241` (et 251-256,
  364-369, 377-384) remplace un `start_time` absent par `FROM_START`, un `stop_time` absent par `UNTIL_END`, et
  **rejette** `start_time > stop_time` (`DatabaseError('You cant create a relation with a stop_time less than the
  start_time')`). Idem `first_seen`/`last_seen` (267-268, 395).
- Sortie STIX : `G/database/stix-converter-utils.ts:25-43` retransforme les sentinelles en `undefined`. Côté front,
  détection textuelle : `F/components/i18n.jsx:10-19` (`isDateStringNone` : vide, `1970…`, `5138…`, `Invalid…`).
- Date « par défaut » d'un objet (front) : `F/utils/defaultRepresentatives.ts:17-37` : cascade `start_time` →
  `first_seen` → `first_observed` → `valid_from` → `published` → `created` → `created_at`. **Un seul instant** par
  objet, aucune incertitude.
- Provenance : `createdBy` (Identity), `objectMarking` (TLP/PAP), `confidence` (`G/schema/attribute-definition.ts:645`),
  `is_inferred` (relations produites par le moteur de règles, dessinées en pointillé :
  `F/components/graph/Graph.tsx:206` `linkLineDash`), `externalReferences`. Versionnage : historique/activité (les
  managers d'activité/audit sont EE), pas de versionnage du graphe d'investigation.

## 4. Patterns UX observés

Captures dans le dépôt (`docs/docs/usage/assets/`) : `investigation.png`, `investigation-workspace.png`,
`investigation-expand-entity.png`, `investigation-rollback-popup.png`, `investigation-create-relationship.png`,
`investigation-export.png`, `investigation-bottom-right-menu.png`, `top-menu-investigation.png`,
`investigation-turn-to-report-or-case*.png` (6 fichiers), `Select-entities-in-graph.png`, `case-graph.png`,
`case-timeline.png`, `report-timeline-view.png`, `report-graph-view.png`, `graph.png`, `inferred-rel-in-graph.png`,
`cases-list.png`, `cases-observables.png`, `case-applying-template.png`. Captures publiques équivalentes :
https://docs.opencti.io/latest/usage/pivoting/ et https://docs.opencti.io/latest/usage/exploring-cases/ . Autre :
`.github/img/screenshot.png`.

### 4.1 Graphe d'investigation (pleine page + barre d'outils en bas)
- Rendu : `react-force-graph-2d` / `react-force-graph-3d` 1.29.2 (`opencti-front/package.json:107-108`), donc d3-force +
  canvas (2D) ou three.js/WebGL (3D, `three-spritetext`). Bascule 2D/3D dans `F/components/graph/Graph.tsx:144-219`
  (`mode3D ? <ForceGraph3D> : <ForceGraph2D>`), boutons « Enable 3D mode », « vertical/horizontal tree mode » (`dagMode`
  td/lr), « Disable forces », « Fit graph to canvas », « Unfix the nodes »
  (`F/components/graph/components/GraphToolbarDisplayTools.tsx:46-84`).
- Chargement : objets paginés par 500 (`InvestigationGraph.tsx:690`), pages suivantes enchaînées automatiquement avec
  debounce 50 ms (`:600, 634-646`) ; barre de progression tant que `loadingCurrent < loadingTotal` ; les forces ne
  démarrent qu'à la fin du chargement (`Graph.tsx:89-105`, `cooldownTicks = 0` pendant le chargement, sinon 100 ticks).
- Positions : drag d'un nœud → fige `fx/fy` puis sauvegarde de **toutes** les positions dans `graph_data` (base64) par
  mutation (`Graph.tsx:110-118` ; `InvestigationGraph.tsx:476-486`). L'état d'affichage (mode 3D, filtres, zoom,
  intervalle) est dans l'URL + localStorage (`GraphState`, `F/components/graph/graph.types.ts:71+` ; exemple d'URL dans
  l'issue #9492).
- Sélection : clic = toggle, Shift+clic = multi ; rectangle libre et lasso (`components/RectangleSelection.tsx`,
  `LassoSelection.tsx`), « Select by entity type », « Select all nodes », sélection des relations enfants/parents des
  nœuds sélectionnés (`GraphToolbarSelectTools.tsx:34-87`). Les non-sélectionnés passent à `globalAlpha 0.3`
  (`utils/useGraphPainter.ts:57`). La recherche texte colore les correspondances et grise le reste (doc pivoting «
  Select / search entity »). Panneau de détails à droite dès qu'il y a sélection (`Graph.tsx:143`,
  `EntitiesDetailsRightBar.tsx`).
- Badge sur chaque nœud : nombre d'éléments liés non affichés, « approximatif, d'où le + » (doc) ; dessiné si
  `showNbConnectedElements` en contexte investigation (`Graph.tsx:209-211`, `useGraphPainter.ts:89`) ; compte calculé
  côté back (`numberOfConnectedElement`) ou, pour méta-objets/identités, par une agrégation dédiée
  (`investigations/utils/fetchMetaObjectsCount.ts:6-37`, uniquement les relations entrantes — limite assumée en
  commentaire `InvestigationGraph.tsx:406-412`).
- Création de relation dans le graphe = création **dans la base de connaissance** (doc « Relationship creation ») ; l'ID
  est ajouté à l'investigation (`InvestigationGraph.tsx:565-572`).

### 4.2 Expansion avec choix des types
- Déclencheurs : bouton 4 flèches (désactivé sans sélection) ou **double-clic** (< 500 ms) sur un nœud, seulement en
  contexte `investigation` (`Graph.tsx:120-134`).
- Dialogue (`F/private/components/workspaces/investigations/InvestigationExpandForm.tsx`) : deux listes de cases à
  cocher « All types of target » et « All types of relationship », chaque option suffixée du **nombre restant à
  ajouter** ; interrupteur « Reset filters » (défaut vrai, `:367, 393`) ; boutons Cancel/Expand.
  - 3 agrégations au backend (`:22-65`) : distribution des types de cibles en sortie, en entrée, et des types de
    relation autour de la sélection.
  - Soustraction de ce qui est déjà affiché : comptage des liens présents par nœud et par type (`:143-216`), puis
    `valeur = total − déjà affichés`, options à 0 masquées, refs pointant vers `User` exclues (`:219-276`, `:242`,
    `:335`).
- Exécution (`F/components/graph/components/GraphToolbarExpandTools.tsx:612-656`) : pour chaque ID sélectionné, **une
  requête séquentielle** `stixRelationships(filters)` avec un groupe OR (`fromId=id ET toTypes ET relationship_type`) OU
  (`toId=id ET fromTypes ET relationship_type`) (`:547-574`) ; on garde la relation et l'autre extrémité si absentes ;
  puis `onInvestigationExpand(allNewElements)`.

### 4.3 Rollback de l'expansion
- Bouton Undo (« Restore the state of the graphic before the last expansion »), désactivé si aucune expansion dans la
  pile (`GraphToolbarExpandTools.tsx:661-667`).
- Confirmation (`investigations/dialog/InvestigationRollBackExpandDialog.tsx:104-127`) : affiche la date de la dernière
  expansion et avertit « All add or remove actions done on the graph after the last expansion will be lost ».
- Mécanisme : §5.1. Le résultat est envoyé au serveur en `replace` de `investigated_entities_ids`
  (`InvestigationGraph.tsx:545-563`).

### 4.4 « Transformer en dossier » (Add to container) et l'inverse
- Menu kebab du workspace → « Add to container » (`F/private/components/workspaces/WorkspaceKebabMenu.tsx:229`,
  `WorkspaceTurnToContainerDialog.tsx:151`) : combobox de recherche de containers existants (types Grouping, Incident
  Response, Report, RFI, RFT) + bouton « Create » qui ouvre le formulaire de création à la volée ; mutation
  `containerEdit(id).knowledgeAddFromInvestigation(workspaceId)` (`:50-62`). Redirection vers l'onglet Knowledge du
  container (doc pivoting « Turn investigation into a container »).
- Inverse : sur un container, bouton « Start an investigation » (`F/components/ExportButtons.jsx:254-262`) → crée le
  workspace et navigue dedans.
- Export : PDF/image et bundle STIX (`F/private/components/workspaces/workspaceHeader/WorkspaceHeader.tsx:41, 79-80`).

### 4.5 Filtre temporel du graphe (« time range »)
- Bouton « Display time range selector » (`GraphToolbarFilterTools.tsx:61`) : la barre d'outils passe de 54 à 134 px
  (`GraphToolbar.tsx:67`) et affiche `GraphToolbarTimeRange` (`components/GraphToolbarTimeRange.tsx:44-106`) : un
  **nuage de points recharts** (100 bacs, taille du point ∝ nombre de relations datées dans le bac, tooltip date +
  compte) superposé à un **slider à deux poignées** (`F/components/range_slider/RangeSlider.jsx`, 15 graduations).
  Déplacer une poignée appelle `setSelectedTimeRange`.
- Effet (`utils/useGraphFilter.ts:23-50`) : une relation hors fenêtre devient `disabled` (dessinée couleur fond) et
  **son nœud cible** est grisé aussi ; les nœuds restent en place (estompage, pas suppression). Mêmes mécanismes pour
  les filtres types d'entité / marquages / auteurs (`GraphToolbarFilterTools.tsx:73-134`).

### 4.6 Timeline de container (Incident Response, RFI, RFT, Report)
- `F/private/components/cases/case_incidents/IncidentKnowledgeTimeLine.jsx:67-116` : `@mui/lab` `<Timeline
  position="alternate">`, **verticale**, un item par objet (date à gauche, pastille icône colorée par type, carte nom +
  description à droite, lien vers l'entité ou la relation).
- Barre du bas `F/private/components/common/containers/ContainertKnowledgeTimeLineBar.tsx:30-51` : recherche, bascule «
  afficher les relations », bascule « date fonctionnelle », filtres (`entity_type`, `objectMarking`, `objectLabel`,
  `createdBy`, `relationship_type`).
- Choix de la date (`IncidentKnowledge.jsx:138-148`) : par défaut `created_at` (date technique d'ingestion) ; « date
  fonctionnelle » → `created` pour les entités, `start_time` pour les relations. Requête `objects(all: true, orderBy…)`
  (`IncidentKnowledgeTimeLine.jsx:152-158`). Pas de durée, pas de lignes par acteur, pas de zoom.

## 5. Algorithmes / mécanismes réutilisables

### 5.1 Pile d'opérations et rollback jusqu'à la dernière expansion
Source : `investigations/utils/useInvestigationState.ts:4-78`, `F/utils/sessionStorage.ts:31-44`,
`InvestigationGraph.tsx:526-563`.
```
Op = Expand{t, ids} | Add{t, ids} | Remove{t, objets_complets}
push(op): pile = [op] + pile ; tronquer à 20 ; sessionStorage["investigation-state-"+id] = pile
on remove(ids):  push(Remove{objets retirés complets}) ; serveur: remove ids
on expand(objs): push(Expand{ids}) ; serveur: add ids
on addRelation:  push(Add{[rel.id]})
rollback():
  ops = dépiler depuis le sommet jusqu'à et y compris le 1er Expand   # O(k)
  garder = objets courants
  pour op dans ops (du plus récent au plus ancien):
     Expand|Add -> garder = garder \ op.ids
     Remove     -> garder = garder ∪ op.objets
  serveur: replace(investigated_entities_ids, ids(garder)) puis reconstruire le graphe
```
Complexité O(k·n) (filtres `includes` sur tableaux). Paramètres : profondeur 20 ops (tous types confondus ; la doc
annonce « last 10 expand actions »). Cas limites : la pile est en **sessionStorage** (perdue à la fermeture de l'onglet,
non partagée entre collaborateurs alors que l'état serveur l'est) ; `Remove` stocke l'objet complet pour pouvoir le
ré-insérer ; un objet ré-ajouté par une autre personne entre-temps est ré-écrasé par le `replace` global.

### 5.2 Comptage « restant à expandre »
`InvestigationExpandForm.tsx:143-276` : `restant(type) = agrégation_backend(type, sélection) − |liens déjà affichés de
ce type touchant la sélection|`, options à 0 masquées, tri décroissant. O(L) sur les liens affichés + 3 agrégations ES.
Limite : la soustraction est approximative quand un même voisin est relié à plusieurs nœuds sélectionnés (d'où le « + »
de la doc et l'issue #9492 : 41 annoncés, 32 ajoutés).

### 5.3 Histogramme + intervalle du slider temporel
`F/components/graph/utils/graphTimeRange.ts:21-88` :
```
dates = [defaultDate(l) pour l dans liens si l est une relation et date non sentinelle], triées
si |dates| = 0 : [now−1j, fin du jour]   si 1 : [d−1j, d+1j]   sinon [min−1j, max+1j]
pas = ceil(minutes(intervalle)/100) ; 100 bacs ; bac_i = |{d : t_i ≤ d ≤ t_i+pas}|
```
O(100·N) (filtre par bac, sans tri exploité). Le filtre (`useGraphFilter.ts:28-33`) désactive une relation si
`defaultDate ∉ [a, b]` ; le test sur `start_time`/`stop_time` y figure mais ces champs ne sont pas copiés dans
`GraphLink` (`graph.types.ts:4-24`, `useGraphParser.ts:188-208`) : en pratique seul l'**instant** `defaultDate` compte,
pas le chevauchement d'intervalles.

### 5.4 Dates sentinelles + garde d'ordre
`data-builder.js:234-241` : `start ← start ?? 1970`, `stop ← stop ?? 5138`, refus si `start > stop`. Avantage : requêtes
de chevauchement sans gestion de NULL (`start ≤ b AND stop ≥ a`). Coût : chaque consommateur doit re-filtrer les
sentinelles (`stix-converter-utils.ts:25-43`, `i18n.jsx:10-19`, `humanize.ts:30`, `ai/dataResolutionHelpers.js:236`).

## 6. Trois patterns à reprendre dans PNEX

### P1. Expansion « à facettes avec compte restant » + double-clic
- Quoi : double-clic sur un nœud → panneau listant les types de relations/rôles et types d'objets voisins avec « (n) »
  restant, cases à cocher, puis Expand (§4.2, §5.2).
- Pourquoi : évite l'expansion aveugle qui noie le graphe (motivation explicite du rollback dans la doc) ; l'utilisateur
  voit le coût avant de cliquer.
- Transposition : endpoint `GET /api/v1/cases/{id}/graph/expand-facets?ids=…` qui fait **une** requête agrégée Postgres
  sur `resource_edges` (`GROUP BY kind, other.type` avec `valid_to IS NULL OR valid_to > now()` ou borné à la fenêtre du
  dossier), en excluant côté serveur les IDs déjà dans le graphe (passés en paramètre) → compte exact, pas
  d'approximation front. Puis `POST …/expand {ids, edge_kinds, object_types}` qui exécute **une seule** requête `WHERE
  from_id = ANY($1) OR to_id = ANY($1)` (pas une requête séquentielle par nœud comme OpenCTI,
  `GraphToolbarExpandTools.tsx:628-633`), avec un plafond (ex. 200 voisins) et un retour « tronqué : n restants ».
  Dioxus : composant `ExpandFacets` avec signaux par case cochée ; libellés via `t!`.

### P2. Journal d'opérations du graphe + rollback « jusqu'à la dernière expansion »
- Quoi : chaque action sur le graphe (expand, add, remove) est une opération typée ; Undo défait tout jusqu'à la
  dernière expansion, avec confirmation datée (§4.3, §5.1).
- Pourquoi : le PRD exige « expansion de nœud avec rollback » ; le découpage par expansion correspond au geste mental («
  reviens avant que j'explore »).
- Transposition : **persister le journal côté serveur** (table `case_graph_ops(case_id, seq, actor_id, kind, payload
  jsonb, at)` ou événements dans O2 comme le journal des changements existant), pas en sessionStorage. Le graphe courant
  = projection du journal ; rollback = nouvelle op `Revert{until_seq}` (append-only, traçable, compatible avec le gel
  SHA-256). Concurrence optimiste : l'op porte `expected_seq` → 409 si un autre membre a écrit entre-temps (règle
  maison). Seuls les **membres d'affichage** du dossier sont touchés ; les relations créées dans l'ontologie ne sont
  jamais supprimées par un rollback (OpenCTI fait pareil : la création de relation est dans la base, le rollback ne
  retire que de l'investigation).

### P3. Slider de période avec histogramme de densité + estompage (pas suppression)
- Quoi : sous le graphe, un histogramme de 100 bacs des éléments datés, avec un intervalle à deux poignées ; hors
  fenêtre = grisé, positions conservées (§4.5).
- Pourquoi : le PRD demande « estompage temporel » et une frise synchronisée ; garder les nœuds en place évite que le
  layout saute pendant le scrub.
- Transposition : la frise du haut PNEX est la source unique du curseur/période (signal global Dioxus `CaseTime {
  cursor, range }`) ; le graphe lit ce signal et calcule `opacity` par nœud/arête. Corriger le défaut OpenCTI : tester
  le **chevauchement d'intervalles** `valid_from ≤ b && (valid_to ≥ a || valid_to is null)` sur les liens temporels de
  `resource_edges` et sur les événements (instant = intervalle dégénéré), pas un instant unique. Histogramme calculé
  côté client en O(N log N) (tri puis balayage) ou par `date_bin` Postgres si N grand. Les objets sans date restent
  pleinement visibles (ce sont les « pièces non datées » du bac).

## 7. Trois pièges à éviter

### Ψ1. Rendu force-directed sans garde-fous de volume
- Problème : le graphe sature le navigateur dès quelques centaines d'éléments. Issues : #1437 « Enhance the large graph
  performances » (2021), #9293 « Analyses graph performance issue when too many objects » (« can even crash the platform
  »), #10586 « particles effect … large graphs are too heavy in CPU », #9298 (rework du chargement), épique #7448 encore
  ouverte. Dans le code, la garde censée couper les labels d'arêtes au-delà de 200 liens est cassée par la précédence
  des opérateurs : `const shouldDisplayLinks = graphData?.links.length ?? 0 < 200;` (`F/components/graph/Graph.tsx:107`)
  vaut `length ?? (0 < 200)` → les labels sont toujours peints dès qu'il y a un lien. Le mode 3D (three.js) ajoute WebGL
  + sprites texte par nœud.
- Parade PNEX : plafond explicite de nœuds affichés (ex. 300) avec message « n masqués » et regroupement par type ;
  labels d'arêtes uniquement au survol/sélection ou au-delà d'un zoom ; layout calculé par simulation bornée (N ticks
  puis arrêt, positions persistées) ; pas de 3D ; un test unitaire sur le seuil de rendu des labels.

### Ψ2. Un seul instant par objet, sentinelles, et appartenance non datée
- Problème : la timeline et le slider réduisent tout objet à `defaultDate` (cascade `start_time → … → created_at`,
  `F/utils/defaultRepresentatives.ts:17-37`) ; par défaut la timeline de case trie sur `created_at` (date d'ingestion,
  `IncidentKnowledge.jsx:138`) — ce qui produit une chronologie de saisie, pas des faits. Les durées (`stop_time`) ne
  sont jamais dessinées, l'incertitude n'existe pas, la ref `object` est `datable: false`
  (`stixRefRelationship.ts:749`). Issues ouvertes : #6079 (« be able to control the date displayed per entity… to have a
  real and meaningful view »), #3903 (timeline plus personnalisable), #2727 (timeline horizontale), #4316 (notes
  absentes de la timeline IR). La doc case-management avoue « we are currently working a more meaningful Timeline view
  ». Les sentinelles 1970/5138 obligent chaque consommateur à les filtrer par chaîne (`i18n.jsx:10-19`).
- Parade PNEX : l'événement daté est l'objet central (PRD) avec `start`, `end` nullable et un champ d'incertitude
  explicite ; NULL = inconnu (Postgres `tstzrange` avec bornes infinies si besoin de requêtes de chevauchement, plutôt
  que des dates magiques) ; jamais `created_at` comme date d'affichage par défaut ; la frise dessine les périodes en
  barres et les instants en points, horizontale.

### Ψ3. État de travail éclaté entre serveur, sessionStorage et URL ; ponts « copie » sans lien
- Problème : la liste des membres est sur le serveur (partagée), la pile de rollback en sessionStorage (locale, perdue à
  la fermeture, 20 ops, alors que la doc dit 10 expansions), l'état d'affichage dans l'URL/localStorage. Un collègue qui
  ouvre l'investigation ne peut pas défaire, et un rollback fait un `replace` complet de `investigated_entities_ids`
  (`InvestigationGraph.tsx:558-561`) qui écrase les ajouts concurrents. « Add to container » ne fait qu'**ajouter** des
  refs (`container.js:258-266`) : pas de lien persistant, positions perdues (issue ouverte #15058 « graph node positions
  are reset when adding an investigation graph to a container »), Notes et Tasks silencieusement exclues
  (`container.js:251`). Erreurs historiques sur ce chemin : #4891 « Error when sending objects from an Investigation to
  a Container », #4238 « Invalid ids specified » à l'expansion. Expansion : N requêtes séquentielles (une par nœud
  sélectionné, `GraphToolbarExpandTools.tsx:628-633`) et comptes incohérents (#9492).
- Parade PNEX : un seul objet « Dossier » qui **est** l'espace de travail (pas de couple investigation/case à
  synchroniser) ; journal d'ops serveur + `expected_version` ; layout stocké avec le dossier et inclus dans le gel ; si
  un export vers un autre dossier existe, il copie aussi positions et rôles et liste ce qui est exclu.

## 8. Réponses aux questions ouvertes du PRD

- (a) **Traces importées CSV/GPX vs flux** : OpenCTI n'a ni géotrace ni série temporelle ; tout est objet/relation
  importé (connecteurs, CSV mappers) puis référencé par ID dans le container (`object_refs`), jamais recopié dans le
  container. Enseignement transposable : le dossier référence la source (device, fichier importé) et une fenêtre, il ne
  copie pas les points — cohérent avec « traces lues à la volée, jamais copiées ». Les dates sentinelles montrent qu'il
  faut distinguer explicitement « inconnu » de « ouvert » dès l'import.
- (b) **Graphe natif vs Cytoscape.js en wasm** : OpenCTI utilise `react-force-graph` (d3-force + canvas/WebGL) et paie
  le prix de la perf (issues §7 Ψ1) malgré des optimisations (chargement paginé, forces gelées pendant le chargement).
  Ce qu'il fait réellement — layout force/arbre, sélection rectangle/lasso, estompage, badges, drag avec positions
  persistées — est modeste ; il n'exploite ni compound nodes ni algorithmes de graphe (demandés dans #6478, #7448,
  ouverts). Pour PNEX, à volume de dossier (dizaines à quelques centaines de nœuds), un rendu natif SVG/canvas en Dioxus
  avec une simulation de forces simple suffit et évite le pont JS ; Cytoscape ne se justifierait que pour des layouts
  avancés (cose-bilkent, compound) — OpenCTI montre qu'on peut s'en passer.
- (c) **Nommage** : OpenCTI sépare « Investigation » (espace de pivot exploratoire, privé par défaut, menu
  Investigations en haut à droite) et « Case » (Incident Response / Request for Information / Request for Takedown,
  objet de connaissance avec tâches, statut, assignés), avec un pont explicite « Add to container » / « Start an
  investigation ». Cette séparation crée la friction de synchronisation décrite en Ψ3. Pour PNEX, un seul objet : «
  Dossier d'enquête » (fr) / « Case » (en) pour l'objet, et « Enquête » réservé au mode de travail (graphe/frise) plutôt
  qu'à un second objet ; éviter « Projet » (déjà ambigu avec les workspaces/dashboards).

## 9. Sources

Fichiers lus (racine `opencti-platform/` sauf mention) :
- `bench/opencti/LICENSE`
- `opencti-graphql/src/enterprise-edition/` (liste), grep de l'en-tête « Enterprise Edition License » sur
  `opencti-graphql/src` et `opencti-front/src`
- `opencti-graphql/src/modules/workspace/workspace.ts`, `workspace.graphql`, `workspace-domain.ts`,
  `investigation-domain.ts`, `workspace-graphql.ts`
- `opencti-graphql/src/domain/container.js` (247-286), `opencti-graphql/src/resolvers/container.js` (1-95),
  `opencti-graphql/config/schema/opencti.graphql` (investigationAdd)
- `opencti-graphql/src/modules/case/case.ts`, `case-incident/case-incident.ts`, `case-rfi/case-rfi.ts`, `case-rft/case-rft.ts`
- `opencti-graphql/src/schema/stixRefRelationship.ts` (736-752), `schema/attribute-definition.ts` (171, 645)
- `opencti-graphql/src/modules/attributes/stixCoreRelationship-registrationAttributes.ts`,
  `stixSightingRelationship-registrationAttributes.ts`, `stixRefRelationship-registrationAttributes.ts`,
  `internalRelationship-registrationAttributes.ts`, `stixDomainObject-registrationAttributes.ts`,
  `modules/event/event.ts`
- `opencti-graphql/src/utils/format.js` (55-75), `database/data-builder.js` (234-268, 364-395),
  `database/stix-converter-utils.ts` (1-43)
- `opencti-front/package.json`
- `opencti-front/src/private/components/workspaces/investigations/InvestigationGraph.tsx`,
  `InvestigationExpandForm.tsx`, `utils/useInvestigationState.ts`, `utils/fetchMetaObjectsCount.ts`,
  `dialog/InvestigationRollBackExpandDialog.tsx`
- `opencti-front/src/private/components/workspaces/WorkspaceTurnToContainerDialog.tsx`, `WorkspaceKebabMenu.tsx`,
  `workspaceHeader/WorkspaceHeader.tsx`
- `opencti-front/src/components/graph/Graph.tsx`, `GraphToolbar.tsx`, `graph.types.ts`,
  `components/GraphToolbarExpandTools.tsx`, `components/GraphToolbarTimeRange.tsx`,
  `components/GraphToolbarDisplayTools.tsx`, `components/GraphToolbarSelectTools.tsx`,
  `components/GraphToolbarFilterTools.tsx`, `utils/graphTimeRange.ts`, `utils/useGraphFilter.ts`,
  `utils/useGraphParser.ts`, `utils/useGraphPainter.ts`
- `opencti-front/src/utils/sessionStorage.ts`, `utils/defaultRepresentatives.ts`, `components/i18n.jsx` (1-30),
  `components/ExportButtons.jsx` (205-265)
- `opencti-front/src/private/components/cases/case_incidents/IncidentKnowledge.jsx`, `IncidentKnowledgeTimeLine.jsx`,
  `CaseIncidentCreation.tsx` (grep EE), `common/containers/ContainertKnowledgeTimeLineBar.tsx`,
  `components/dashboard/WidgetTimeline.tsx`
- Doc dans le dépôt : `docs/docs/usage/pivoting.md`, `docs/docs/usage/case-management.md`,
  `docs/docs/usage/exploring-cases.md` (grep timeline), `docs/docs/usage/assets/` (liste des captures)

URLs consultées :
- https://docs.opencti.io/latest/usage/pivoting/ (et versions 6.x, même contenu)
- https://github.com/OpenCTI-Platform/opencti/issues/1437
- https://github.com/OpenCTI-Platform/opencti/issues/9293
- https://github.com/OpenCTI-Platform/opencti/issues/10586
- https://github.com/OpenCTI-Platform/opencti/issues/9298
- https://github.com/OpenCTI-Platform/opencti/issues/7448
- https://github.com/OpenCTI-Platform/opencti/issues/6478
- https://github.com/OpenCTI-Platform/opencti/issues/15058
- https://github.com/OpenCTI-Platform/opencti/issues/9492
- https://github.com/OpenCTI-Platform/opencti/issues/4891
- https://github.com/OpenCTI-Platform/opencti/issues/4238
- https://github.com/OpenCTI-Platform/opencti/issues/1230
- https://github.com/OpenCTI-Platform/opencti/issues/6079
- https://github.com/OpenCTI-Platform/opencti/issues/3903
- https://github.com/OpenCTI-Platform/opencti/issues/2727
- https://github.com/OpenCTI-Platform/opencti/issues/4316
- https://github.com/OpenCTI-Platform/opencti/issues/4314
