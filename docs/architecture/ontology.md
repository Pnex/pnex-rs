# PRD — PNEX 0.2.0 : noyau ontologique (objets, liens, temps, actions) (D176–D191)

**Statut :** Proposé (2026-10-09) — cible de la version **0.2.0**, première
version réécrite autour de ce noyau. Zéro code avant validation.
**Portée :** transverse — modèle de domaine, API, UI, assistant, flows,
dashboards. Tous les domaines existants deviennent des vues spécialisées
de ce noyau.
**Renvois :** `inventory.md` D42 (couche Resource : labels, containment,
edges — la graine de ce PRD), D43 (placements), D69 (recherche),
`media-ingest.md` (D159–D175 : flux, plages, taxonomies),
`viz-bases.md` (liaisons de lecture), `ai-assistant.md` (D123, D142–D145),
`worker-fabric.md` (as-code / as-UI sur une même source de vérité),
`edge-model.md`, `security.md` (R1–R20).

> **Numérotation** : PRD rédigé en D165–D180 ; renuméroté D176–D191 à
> l'intégration (D148–D158 déjà pris par `security-tiers.md`, puis
> `media-ingest.md` en D159–D175).

---

## 1. Constat

PNEX a commencé comme plateforme IoT. Il porte aujourd'hui des devices,
des flows, des dashboards, des médias, des tours, des POI, des
annotations, des caméras, des modèles ML, et bientôt des flux média, des
plages et des taxonomies. Chaque domaine a ses tables, ses pages et sa
façon de se relier aux autres.

Le noyau qui émerge est le même partout : **capter des sources, les
ramener sur un axe temporel, les rattacher à des objets du monde réel,
les analyser, agir.** C'est la forme d'une plateforme de données
opérationnelle (catégorie Palantir Foundry / ArgonOS), mais PNEX n'en a
pas encore la pièce centrale : un **modèle d'objets métier défini par
l'utilisateur**.

Ce qui existe déjà et sert de graine (D42) :

- une identité polymorphe `ResourceRef (org, kind, id)` ;
- des **labels**, un **containment** (arbre) et des **edges** génériques
  (`resource_edges`, relation `placed_on`) ;
- un registre `KindSpec` où la validité kind × relation × kind est
  déclarée par kind, sans `match` dans le moteur.

Ce qui manque :

1. Les kinds sont **codés en dur** (7 constantes). Un utilisateur ne peut
   pas créer « Pompe », « Ligne de production » ou « Chaîne TV ».
2. Aucun modèle de **propriétés typées** : le métier vit dans des JSONB
   `metadata` libres.
3. Le **temps est rattaché aux devices**, pas aux objets : remplacer un
   capteur casse l'historique de la pompe qu'il mesurait.
4. Les liens servent à **ranger**, pas à **raisonner** : pas de
   relation typée, pas de validité temporelle, pas de provenance.
5. Pas de notion d'**action** métier déclarée et tracée.

## 2. Vision 0.2.0

> **Tout ce que PNEX connaît est un objet typé, relié à d'autres objets,
> porteur de temps, d'où l'on peut agir, et dont chaque fait dit d'où il
> vient.**

Cinq primitives, et rien d'autre dans le noyau :

| Primitive | Question à laquelle elle répond |
|---|---|
| **Objet** | De quoi parle-t-on ? (une pompe, un site, une chaîne, une personne) |
| **Lien** | Comment ces choses sont-elles reliées, et depuis quand ? |
| **Temps** | Que s'est-il passé, quand, et sur quelle plage ? |
| **Action** | Que peut-on faire sur cet objet, qui l'a fait, avec quel résultat ? |
| **Provenance** | D'où vient chaque fait, et peut-on le rejouer ? |

Positionnement : une plateforme **de l'edge à l'ontologie**, open source
(MIT), souveraine, qui tient sur un Raspberry Pi en tier sqlite et monte
en cluster en tier Postgres. Le capteur à 3 € et l'objet métier vivent
dans le même modèle.

## 3. Non-objectifs

- La parité fonctionnelle avec Palantir ou ArgonOS.
- Une base graphe dédiée : le tier sqlite (Pi, tout-en-un) l'interdit ;
  tout reste portable PG/sqlite (école D69 : pas de SQL spécifique non
  gardé).
- Un environnement de code de type notebook ou workbook : le calcul
  passe par les flows et les fonctions existants.
- La fédération inter-orgs (partage d'objets entre organisations) : plus
  tard.
- La prise de décision par l'IA : l'assistant propose, l'humain ou un
  flow déployé agit (D123 inchangé).

## 4. Décisions

### D176 — Les types d'objets sont des données, versionnées

- `object_types` (org-scoped, ou `org_id NULL` pour les types système) :
  `key` (`[a-z0-9_]`), `name`, `icon`, `system` (bool), `latest_version`.
- `object_type_versions` (append-only, école D18) : schéma des
  propriétés (D178), règles de containment (D180), actions (D183),
  `expected_version` → 409 à l'écriture.
- Les **types système** (`device`, `flow`, `dashboard`, `media_asset`,
  `tour`, `map_pin`, `folder`, puis `media_stream`, `time_range`,
  `ml_model`…) sont déclarés dans ce même registre, `system = true`,
  non supprimables, schéma extensible par l'org (propriétés
  supplémentaires) sans toucher aux colonnes natives.
- Le registre `KindSpec` (D42) n'est plus écrit à la main : il est
  **dérivé** du registre de types au démarrage. Les constantes
  `KIND_*` restent pour le code qui manipule un type système précis.

### D177 — Stockage hybride : objets utilisateur en table générique, objets système dans leurs tables

- Types utilisateur → table `objects` (UUID, `org_id`, `type_id`,
  `type_version`, `title`, `properties` JSONB, `source_ref` (D184),
  `created_at`, `updated_at`, `archived_at`).
- Types système → **inchangés dans leurs tables** (`device_registries`,
  `flows`, `media_assets`…). Ils sont exposés comme objets par un
  **adaptateur** par type (lecture des colonnes natives + propriétés
  supplémentaires de l'org dans `object_extensions (kind, id,
  properties)`).
- Pourquoi : déplacer les devices dans une table générique casserait les
  chemins chauds (`/ws/device`, firmware, OTA) pour aucun gain. L'API et
  l'UI, elles, voient un modèle **uniforme** par `ResourceRef`.
- Garde-fou anti-EAV : les propriétés sont un **document JSONB validé
  par schéma**, pas une table ligne-par-propriété ; index GIN (PG) sur
  les propriétés déclarées `indexed`, scan Rust en sqlite (école D42).

### D178 — Propriétés typées, y compris les propriétés temporelles

Types de propriétés (schéma partagé dans `pnex-core`, natif + wasm32,
validation identique front et back) :

- scalaires : `text`, `number` (+ unité, réutilise `unit_conversions`),
  `bool`, `date`, `datetime`, `enum`, `geo_point`, `url`, `ref`
  (référence typée vers un autre objet) ;
- **temporelles** :
  - `series` : liaison vers une série O2 (métrique + sélecteur de
    labels) ; la valeur « actuelle » est la dernière valeur, l'historique
    est la série elle-même ;
  - `events` : liaison vers un stream O2 logs + filtre (transcriptions,
    détections, notifications).

Une propriété temporelle ne copie jamais de données : elle **désigne** où
elles vivent. Le moteur de lecture (`series-batch`, `_search`) est
inchangé.

### D179 — Liens typés, temporels, porteurs d'attributs

- `link_types` (org-scoped ou système) : `key`, `name`, `inverse_name`,
  `from_type`, `to_type` (ou `*`), cardinalité (`one`, `many` de chaque
  côté), schéma d'attributs, `temporal` (bool).
- `resource_edges` (D42) devient la table des liens : ajout de
  `link_type_id`, `attributes` JSONB validé, `valid_from` / `valid_to`
  (nullables), `source_ref` (D184). `placed_on` devient le premier type
  de lien système ; la colonne `placement` reste son attribut.
- **Validité temporelle** : « le capteur C est monté sur la pompe P du
  1ᵉʳ mars au 12 juin », « le groupe G possède la chaîne X depuis 2015 ».
  Une requête « à la date T » ne voit que les liens valides à T. Fermer
  un lien (poser `valid_to`) ne le supprime jamais : l'histoire reste.
- La purge symétrique `purge_for` (D42) est conservée.

### D180 — Le containment reste un arbre, séparé des liens

Le containment (un seul parent, cycles refusés, D42) sert la hiérarchie
physique et organisationnelle : site › bâtiment › ligne › machine ; org
› dossier. Les règles « peut contenir / peut être contenu dans » passent
du code au schéma de type (D176). Les liens (D179) servent tout le
reste. Ne pas fusionner les deux : un arbre se navigue, un graphe se
requête.

### D181 — Le temps appartient aux objets, pas aux devices

- **Liaison device → objet** : une capability d'un device est liée à une
  propriété `series` d'un objet (« température de C → P.température »).
  La liaison est un lien temporel (D179) : remplacer le capteur ferme
  l'ancienne liaison et en ouvre une nouvelle.
- La lecture de `P.température` **recompose** l'historique à partir des
  liaisons successives : l'objet survit à ses capteurs. C'est la
  première valeur visible de la 0.2.0 pour l'industrie (maintenance,
  remplacement de matériel, audit).
- Toutes les voies d'ingestion (télémétrie device, agent edge D95, flux
  média D163, `http-fetch`) peuvent étiqueter leurs points d'un
  `object=<kind>/<id>` : les séries sont alors interrogeables par objet
  sans liaison explicite.

### D182 — Les plages s'appliquent à n'importe quel objet

`time_ranges` (`media-ingest.md` D169) prend pour `scope` n'importe quel
`ResourceRef` : émission d'une chaîne, poste d'équipe d'une ligne, lot
d'une machine, mandat d'une personne. L'agrégation « par plage » (D171)
devient une primitive de lecture du noyau, pas une fonction média.

### D183 — Actions déclarées sur les types, exécutées par des flows, tracées

- Une action est déclarée dans le schéma de type : `key`, `name`,
  schéma de paramètres, préconditions (expression sur les propriétés),
  **exécuteur** = point d'entrée d'un **flow déployé** (nouveau nœud
  `action-trigger`) ou action système (ex. archiver).
- Invocation : UI (bouton sur la page objet), API, flow. Option
  `confirm: true` → confirmation humaine obligatoire.
- Chaque invocation est journalisée dans le stream O2 `actions` : qui,
  quand, objet, paramètres, version du flow, résultat.
- L'assistant IA peut **proposer** une invocation (pré-remplie), jamais
  l'exécuter (D123 : seul un flow déployé par un humain agit).

### D184 — Provenance obligatoire sur chaque fait

- `source_ref` sur chaque objet, valeur de propriété modifiée et lien :
  `manual:<user>`, `import:<job>`, `flow:<id>@<version>`,
  `device:<id>`, `stream:<id>/segment:<id>`, `api:<token>`.
- Historique des modifications d'objets et de liens dans un stream O2
  `object_changes` (avant/après, provenance) : pas de table d'audit en
  base, rétention réglable (D72).
- Page objet : onglet **Provenance** qui répond à « d'où vient ce
  chiffre / ce lien ». C'est la condition pour Repère (fait sourcé et
  rejouable) comme pour l'audit industriel.

### D185 — Une API de requête sur l'ontologie, portable PG/sqlite

`POST /api/v1/ontology/query`, requête JSON déclarative (pas de langage
textuel en V1) :

- filtre par type, propriétés, labels effectifs (D42), containment ;
- traversée de liens **bornée** (profondeur ≤ 4, type de lien explicite,
  date de validité `as_of`) ;
- jointure temporelle : dernière valeur, agrégat sur fenêtre ou sur
  plage (D182) des propriétés `series`.

Implémentation : SQL portable + CTE récursives bornées en PG, parcours
Rust en sqlite (école D42) ; les agrégats temporels sont délégués à O2.
La recherche globale (D69) s'étend aux objets. Le schéma de la requête
vit dans `pnex-core` : l'UI, l'assistant et les flows (nœud
`ontology-query`) parlent le même langage.

### D186 — UI : un explorateur générique, les pages actuelles deviennent des vues

- **Explorateur** : liste par type (filtres D185), page objet
  standard = propriétés, liens (graphe local + liste), panneau temps
  (séries et événements liés), actions, provenance.
- **Éditeur de types** (as-UI) et **export/import YAML** (as-code,
  GitOps) sur la même source de vérité, école `worker-fabric.md` §9.
- Les pages existantes (`/devices`, `/media`, `/cameras`, dashboards,
  tours) restent : ce sont des **vues spécialisées** de types système,
  et chacune gagne un lien vers la page objet générique.
- Un utilisateur hobbyiste n'a jamais besoin de voir l'ontologie : il
  passe par les packs (D190) et les pages spécialisées.

### D187 — Dashboards et rapports se lient aux objets et aux types

- Les liaisons de lecture (`SourceRef`, D129) gagnent une cible
  `object_property` (objet + propriété temporelle).
- **Dashboard de type** : un dashboard conçu pour le type « Pompe »
  s'affiche pour n'importe quelle pompe (sélecteur d'objet), au lieu
  d'un dashboard copié par équipement.
- Les rapports (`media-ingest.md` D173) s'appuient sur la même requête
  (D185) : « temps de parole par personnalité sur les plages de type
  émission de la chaîne X » s'exprime dans l'ontologie, pas en code.

### D188 — Permissions : l'org reste la frontière en 0.2.0

La frontière de sécurité reste l'**org** puis la **plateforme** (R1, R2).
En 0.2.0 : rôle par org (inchangé) + verrou par **type** (lecture seule
pour les viewers, écriture réservée aux rôles désignés). Les ACL par
objet et le marquage de sensibilité (exigés par la défense et le public)
sont une décision explicite ultérieure, mais le modèle les anticipe :
toute lecture passe par un point unique (D185) où le filtre pourra se
brancher.

### D189 — L'assistant IA raisonne sur l'ontologie

- Le schéma (types, liens, actions) devient le contexte principal de
  l'assistant : sa base de connaissance est **générée** depuis le
  registre, pas écrite à la main (prolonge D142).
- Outils : lecture via D185 ; création et édition de types, d'objets et
  de liens via le service partagé avec l'UI (`expected_version`, R1, R2,
  D144) ; proposition d'actions (D183), jamais d'exécution.
- Règle CLAUDE.md inchangée : tout nouveau type système, type de lien
  système ou nœud livré sans sa documentation pour l'assistant = travail
  non fini.

### D190 — Packs : la frontière entre le noyau et les applications

Un **pack** est un ensemble importable/exportable (YAML versionné) de
types, types de liens, actions, flows, dashboards de type et modèles de
rapport. Le noyau ne connaît aucun métier ; les métiers sont des packs.

Packs de référence pour valider le noyau :

- **Maintenance augmentée** (vedette `use-cases.md`) : Site, Ligne,
  Machine, Pompe ; liaisons capteurs ; action « mettre en
  maintenance » ; dashboard de type Pompe.
- **Maison** : Pièce, Équipement ; reprend la palette domotique P2.12.
- **Couverture médiatique** (Repère) : Chaîne, Groupe, Émission (plage),
  Personnalité, Organisation, Sujet ; liens temporels « possède »,
  « présente », « intervient dans », « rattaché à » ; provenance jusqu'au
  segment de transcription.

Règle : une fonctionnalité qui n'a de sens que pour un pack va **dans le
pack**, jamais dans le noyau.

### D191 — Migration 0.1 → 0.2 : non destructive

La 0.1 est publiée (2026-09-27) : contrairement à D42 (« app non prod,
migration destructrice »), la 0.2.0 **migre les données**.

- Les tables système sont conservées ; elles sont enregistrées comme
  types système (D176).
- `resource_edges` est étendue en place (D179) ; les lignes `placed_on`
  reçoivent le type de lien système correspondant.
- `device_placements` (D43) est exprimé comme lien système `placed_at`
  (device → map_pin, cardinalité un) ; la table peut rester comme index
  matérialisé si les performances l'exigent.
- API : les routes `/api/v1/resources/*` restent servies pendant toute
  la 0.2.x (adaptateur), marquées dépréciées ; `CONTRACT` est incrémenté
  pour les nouvelles routes `/api/v1/ontology/*`.
- Migration testée sur une base 0.1 réelle (fixture) en PG **et** sqlite.

## 5. Modèle de données (récapitulatif)

| Table / stream | Rôle |
|---|---|
| `object_types` / `object_type_versions` | registre des types (D176) |
| `objects` | objets des types utilisateur (D177) |
| `object_extensions` | propriétés supplémentaires des objets système (D177) |
| `link_types` | registre des types de liens (D179) |
| `resource_edges` (étendue) | liens typés, temporels, attribués (D179) |
| `resource_containments`, `resource_labels`, `resource_folders` | inchangées (D42, D180) |
| `time_ranges` | plages sur tout objet (D182) |
| `packs` / `pack_versions` | packs installés (D190) |
| O2 `object_changes` | historique et provenance (D184) |
| O2 `actions` | journal des invocations (D183) |

## 6. Exemple bout en bout (pack maintenance)

1. L'utilisateur installe le pack : types Site › Ligne › Machine ›
   Pompe, type de lien `mesure` (capability → propriété), action
   « Mettre en maintenance ».
2. Il crée la pompe P12 dans la ligne L2, lie le capteur C47 à
   `P12.température` (lien temporel ouvert aujourd'hui).
3. Le flow d'anomalie (nœud `anomaly`) lit `P12.température` par
   l'ontologie, pas par l'identifiant du capteur.
4. Le capteur est remplacé : C47 → C88. Le lien est fermé et rouvert ;
   la courbe de P12 reste continue, le dashboard de type Pompe ne bouge
   pas.
5. Un technicien invoque « Mettre en maintenance » : le flow notifie,
   l'invocation est journalisée avec sa provenance.

## 7. Lots de la 0.2.0

| Lot | Contenu | Sortie |
|---|---|---|
| **L0** | Spike : types en données dérivant le `KindSpec` existant, sans changement fonctionnel | registre généré identique au registre codé (test d'égalité) |
| **L1** | D176–D178 : types, objets, propriétés scalaires, éditeur de types, YAML | créer « Pompe » et 100 objets en UI et en YAML, PG + sqlite |
| **L2** | D179–D180 : types de liens, validité temporelle, migration `placed_on` / `placed_at` | requête `as_of` correcte sur liens fermés |
| **L3** | D178 temporel + D181 : propriétés `series`/`events`, liaisons device → objet | remplacement de capteur sans rupture de courbe |
| **L4** | D185–D187 : API de requête, explorateur, dashboards de type | dashboard de type Pompe, recherche globale sur objets |
| **L5** | D184, D189, D191 : provenance, assistant, migration 0.1 → 0.2 | migration d'une base 0.1 réelle, onglet provenance |
| **0.2.0** | L0–L5 + pack « Maintenance augmentée » | release |
| **0.3** | D183 actions, packs Maison et Couverture médiatique, ACL par objet (décision) | |

Le PRD `media-ingest.md` s'aligne : `media_stream` et `time_range`
naissent directement comme types système de l'ontologie si leur
implémentation commence après L1.

## 8. Risques

- **Sur-généricité** : un noyau trop abstrait devient lent et illisible.
  Parade : types système dans leurs tables (D177), packs concrets dès la
  0.2.0, pas de langage de requête textuel en V1.
- **Performance du JSONB** sur gros volumes : index GIN uniquement sur
  propriétés `indexed`, mesures sur 100 k objets en PG et 10 k en
  sqlite avant la release.
- **Dispersion** : la 0.2.0 est un chantier de noyau ; aucun nouveau
  pilier fonctionnel ne démarre pendant sa réalisation (seuls les
  correctifs et la finition des piliers existants).
- **Complexité perçue** par les makers : l'ontologie est invisible tant
  qu'on n'en a pas besoin (D186, D190).
- **Comparaison commerciale** : ne pas se vendre comme « Palantir
  open source » tant que l'accompagnement (déploiement, intégration,
  souveraineté documentée) n'existe pas ; vendre d'abord la maintenance
  augmentée sur un noyau ontologique.

## 9. Questions ouvertes

1. Faut-il un identifiant d'objet global unique (UUID pour tous, y
   compris les objets système aux PK `i64`), ou garder `ResourceRef`
   stringifiée (D42) ?
2. Le schéma de propriétés : format maison dans `pnex-core`, ou JSON
   Schema (outillage existant, mais plus lourd à valider en wasm) ?
3. Liaison device → objet : lien temporel générique (D179) ou table
   dédiée pour les performances de recomposition des séries ?
4. Les packs installés sont-ils modifiables par l'org (fork local) ou
   seulement extensibles (surcouche), pour garder les mises à jour du
   pack ?
5. Un langage de requête textuel (type Cypher/GQL réduit) en 0.3, ou
   jamais ?
6. Gouvernance du noyau en vue d'une fondation (CNCF, Apache) : les
   types système et le format des packs sont-ils une spécification
   publique versionnée à part ?
