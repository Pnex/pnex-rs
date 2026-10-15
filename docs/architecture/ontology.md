# PRD — PNEX 0.2.0 : noyau ontologique (objets, liens, temps, actions) (D176–D191)

**Statut :** Implémenté L0–L5 (2026-10-10, §10) — PRD proposé le 2026-10-09, cible de la version **0.2.0**, première
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
(MIT), souveraine, qui tient sur un Raspberry Pi et monte en cluster,
sur PostgreSQL dans les deux cas (décision #19). Le capteur à 3 € et l'objet métier vivent
dans le même modèle.

## 3. Non-objectifs

- La parité fonctionnelle avec Palantir ou ArgonOS.
- Une base graphe dédiée : tout reste dans PostgreSQL (CTE récursives,
  JSONB).
- Un environnement de code de type notebook ou workbook : le calcul
  passe par les flows et les fonctions existants. Amendé le 2026-10-10
  (`pages.md` P7b, décision #21) : le tableau local d'une page est admis
  car une formule ne fait que **lire** (jamais d'écriture ni de code).
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
  les propriétés déclarées `indexed`.
- **Amendé le 2026-10-10 (annexe A2) — identité universelle** : `objects`
  porte une ligne d'**identité** pour *tout* objet, système compris
  (UUID, `org_id`, `type_id`, titre, `valid_from` / `valid_to`) ; le
  contenu natif des types système reste dans sa table, en 1:1 avec FK
  vers `objects.id`. Les liens (D179) ont de vraies FK vers `objects.id`.
  Les objets sont **temporels** comme les liens : `archived_at` devient
  la fermeture de validité (`valid_to`).

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

> Livré pour les séries temporelles (2026-10-10, media-ingest.md §19) :
> `POST /api/v1/telemetry/aggregate` agrège une série O2 (`sum`, `avg`,
> `max`, `min`, `increase`) par plage d'une portée (heure recalée si elle
> existe, sinon annoncée, la base est exposée par ligne) ou par tranche
> horaire répétée sur les jours d'une fenêtre ; le widget de dashboard
> `range_bars` le consomme. Les propriétés `series` des objets (D185) le
> réutiliseront.

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

> **Livré 2026-10-13 (0.2.0, question #17 tranchée : oui)** — écarts au
> texte ci-dessus, assumés :
> - Journal : pas de stream O2 `actions` dédié ; l'invocation est une
>   entrée `action.invoke` du journal de provenance `object_changes`
>   (paramètres, flows destinataires), visible dans l'onglet Provenance.
>   Un seul lecteur, une seule rétention.
> - Pas de préconditions ni d'action système en V1 ; pas de résultat
>   remonté par le flow (fire-and-forget, comme les contrôles D127) : le
>   journal dit « envoyé à ces flows (id, version) ».
> - Bus : `PUBLISH pnex:act:v1:{org}` (Valkey), nœud `pnex-action-trigger`
>   (crate `pnex-node-ui-control`, à côté de `control-source`). Une
>   invocation sans flow déployé tenant le déclencheur est refusée (409
>   `ontology-action-no-flow`) ; le deploy refuse un déclencheur dont
>   l'action n'existe plus (`ontology-action-unknown`).
> - `confirm: true` = avertissement dans le dialogue ; toute invocation
>   UI passe de toute façon par un dialogue et un clic explicite.
> - Garde : `can_write` + `write_role` du type (D188), paramètres validés
>   comme des propriétés (refs vérifiées dans l'org, R1).
> - Assistant : outil `propose_action` (vérifie, liste les flows
>   destinataires, n'envoie rien) ; l'UI affiche « Vérifier et exécuter »
>   qui ouvre le même dialogue pré-rempli.
> - Déclaration : dans le schéma du type (`actions`), via un pack ou
>   l'import YAML ; pas encore d'éditeur d'actions dans l'éditeur de type.

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

### D185 — Une API de requête sur l'ontologie

`POST /api/v1/ontology/query`, requête JSON déclarative (pas de langage
textuel en V1) :

- filtre par type, propriétés, labels effectifs (D42), containment ;
- traversée de liens **bornée** (profondeur ≤ 4, type de lien explicite,
  date de validité `as_of`) ;
- jointure temporelle : dernière valeur, agrégat sur fenêtre ou sur
  plage (D182) des propriétés `series`.

Implémentation : CTE récursives bornées en PostgreSQL ; les agrégats temporels sont délégués à O2.
**Amendé le 2026-10-10 (annexe A3)** : l'API reste bornée (≤ 4) ; en
interne, l'interface graphe de `pnex-core` couvre profondeur variable,
plus court chemin et impact en cascade (CTE, timeout et plafond de
lignes) et `petgraph` pour les algorithmes (annexe A3).
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
- Migration testée sur une base 0.1 réelle (fixture).

### Labels et ontologie : deux rôles, pas deux modèles (2026-10-11)

> Numéro D à attribuer à la validation (D192+ réservé par
> `media-vision-studio.md`).

Les labels D42 restent, mais ne sont **pas** un second outil de
modélisation :

- **Ontologie = ce que l'objet est** : type, propriétés typées, liens
  datés et sourcés. Tout fait métier sur lequel on raisonne, requête ou
  agit y vit.
- **Labels = étiquettes de travail** : clé:valeur libre, sans schéma,
  pour trier, filtrer et regrouper à la volée (« lot:2026 »,
  « à-vérifier »). D185 filtre par labels effectifs : les deux
  cohabitent.
- ~~Les labels ne sont pas étendus à tous les kinds~~ **Révisé par D218**
  (`design-system.md` §7, 2026-10-15) : les étiquettes s'appliquent à tout
  objet listable et alimentent la recherche globale ; structurer un objet
  passe toujours par l'ontologie.
- Un label qui porte un fait métier (`marque:grundfos`, `site:serre`) est
  une dette : la bonne place est une propriété ou un lien. Évolution
  possible, non planifiée : « promouvoir ce label en propriété » (même
  clé sur beaucoup d'objets d'un type → propriété du type, valeurs
  migrées, provenance D184).
- À ne pas confondre avec les labels des séries O2 (`__name__`,
  `device_id`…) : sélecteurs de données, utilisés par les propriétés
  `series` (D178).
- UI : sur une fiche, type et propriétés d'abord ; labels en étiquettes
  discrètes (`LabelTag`, `design-system.md`).

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
| O2 `object_changes` (`action.invoke`) | journal des invocations (D183, amendé) |

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
| **L0** ✅ | Spike : types en données dérivant le `KindSpec` existant, sans changement fonctionnel | registre généré identique au registre codé (test d'égalité) — **livré 2026-10-10** : `pnex_core::ontology` (`ObjectTypeDef`, `LinkTypeDef`, `TypeSet`), `KindSpec` supprimé, garde `derived_registry_matches_d42_rules` |
| **L1** ✅ | D176–D178 : types, objets, propriétés scalaires, éditeur de types, YAML | créer « Pompe » et 100 objets en UI et en YAML — **livré 2026-10-10** (18af2f9d, 21375662) |
| **L2** ✅ | D179–D180 : types de liens, validité temporelle, migration `placed_on` / `placed_at` | requête `as_of` correcte sur liens fermés — **livré 2026-10-10** |
| **L3** ✅ | D178 temporel + D181 : propriétés `series`/`events`, liaisons device → objet | remplacement de capteur sans rupture de courbe — **livré 2026-10-10** |
| **L4** ✅ | D185–D187 : API de requête, interface graphe (CTE + `petgraph`), explorateur, **vue graphe Dioxus**, dashboards de type, verrou par type (D188) | dashboard de type Pompe, recherche globale sur objets — **livré 2026-10-10** (7318940c) |
| **L5** ✅ | D184, D189, D191 : provenance, assistant, migration 0.1 → 0.2 | migration d'une base 0.1 réelle, onglet provenance — **livré 2026-10-10** (e0298159) |
| **0.2.0** | L0–L5 + pack « Maintenance augmentée » (1.1.0 : action `start_maintenance`, D183 livré 2026-10-13) | release |
| **0.3** | packs Maison et Couverture médiatique, ACL par objet (décision) | |

Le PRD `media-ingest.md` s'aligne : `media_stream` et `time_range`
naissent directement comme types système de l'ontologie si leur
implémentation commence après L1.

## 8. Risques

- **Sur-généricité** : un noyau trop abstrait devient lent et illisible.
  Parade : types système dans leurs tables (D177), packs concrets dès la
  0.2.0, pas de langage de requête textuel en V1.
- **Performance du JSONB** sur gros volumes : index GIN uniquement sur
  propriétés `indexed`, mesures sur 100 k objets avant la release.
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

1. ~~Faut-il un identifiant d'objet global unique (UUID pour tous, y
   compris les objets système aux PK `i64`), ou garder `ResourceRef`
   stringifiée (D42) ?~~ **Tranché 2026-10-10** : UUID pour tous via la
   table d'identité universelle (D177 amendé, annexe A2) ; `ResourceRef`
   reste une forme d'affichage et d'API.
2. ~~Le schéma de propriétés : format maison dans `pnex-core`, ou JSON
   Schema ?~~ **Tranché 2026-10-10** : format maison (§10.2), validation
   identique front/back, aucun outillage JSON Schema à embarquer en wasm.
3. ~~Liaison device → objet : lien temporel générique (D179) ou table
   dédiée ?~~ **Tranché 2026-10-10** : lien système générique `measures`
   (§10.3) ; une table dédiée seulement si la recomposition devient un
   goulot mesuré.
4. ~~Les packs installés sont-ils modifiables par l'org ?~~ **Tranché
   2026-10-10** : fork local — les types installés appartiennent à l'org
   et restent éditables ; une mise à jour du pack ajoute une version (§10.5).
5. Un langage de requête textuel (type Cypher/GQL réduit) en 0.3, ou
   jamais ?
6. Gouvernance du noyau en vue d'une fondation (CNCF, Apache) : les
   types système et le format des packs sont-ils une spécification
   publique versionnée à part ?

## Annexe — Ontologie & stockage graphe (2026-10-10)

Synthèse de la discussion du 2026-10-10, intégrée et arbitrée le même
jour. Ne remplace pas les décisions D176–D191 : les amende là où c'est
indiqué.

### A1. Principes

- L'ontologie est **le chantier prioritaire de la 0.2.0** : socle de la
  recherche globale, du graphe de relations, du time travel, des
  permissions fines et du hub de kits (`hub.md`, gelé jusque-là).
- **Ontologie optionnelle** : un device sans objet rattaché fonctionne
  normalement ; l'ontologie s'ajoute par-dessus, elle ne s'impose pas aux
  makers (cohérent avec D186, D190).
- POI + placements = première ontologie existante, **migrés** en types
  intégrés, jamais dupliqués (D191).
- Les données restent où elles sont : télémétrie dans OpenObserve, octets
  dans le MediaStore (fs / S3 RustFS, D21) ; l'ontologie les référence
  par identifiant.
- Le **méta-modèle** (format des schémas, types et liens système,
  interface graphe) vit dans `pnex-core` ; les types d'org sont des
  données (D176).

### A2. Stockage : identité universelle (tranché)

Trois options pesées pour la sérénité à long terme :

| Option | Verdict |
|---|---|
| Tout dans des tables `object` / `link` génériques | Écarté : chemins chauds (`/ws/device`, présence, OTA, build) en JSONB, perte des FK typées et des entités SeaORM, chaque fonctionnalité système revalidée à la main |
| D177 tel qu'écrit (`ResourceRef` chaîne) | Écarté : pas de FK sur les liens, intégrité par purge applicative |
| **Table d'identité pour tout objet, contenu natif en place** | **Retenu** |

Conséquences : une ligne `objects` par entité, système ou utilisateur
(UUID, org, type, titre, `valid_from` / `valid_to`) ; tables système en
1:1 avec FK ; liens avec FK réelles ; création et suppression système
écrivent l'identité dans la même transaction ; la migration 0.1 → 0.2
(D191) crée une ligne d'identité par entité existante. Objets **et**
liens sont temporels (temps de validité). Un déplacement = fermeture
d'un lien + ouverture d'un autre, rien n'est écrasé. Contraintes pour
toute projection graphe future : identifiants stables, un seul type par
lien, source et destination explicites.

### A3. Accès graphe

| Besoin | Mécanisme |
|---|---|
| Parcours (profondeur fixe ou variable, rétroliens, état à une date) | **Jointures et CTE récursives** générées par le cœur Rust, timeout et plafond de lignes |
| Algorithmes (plus court chemin, impact en cascade, cycles, composantes) | **`petgraph`** (Rust, MIT/Apache-2.0) sur le sous-graphe chargé par CTE |
| Filtre temporel | SQL classique sur `valid_from` / `valid_to` |

SQL/PGQ (`CREATE PROPERTY GRAPH` / `GRAPH_TABLE`) n'est **pas dans
PostgreSQL 19** (vérifié sur 19beta4, 2026-10-10) ; il n'apporterait de
toute façon qu'une syntaxe, réécrite en jointures par le planificateur.
Branchements possibles plus tard, jamais prérequis : **pgRouting**
(GPL-2, algorithmes en SQL sur notre table de liens, exige des
identifiants entiers) une fois PostGIS monté, **Apache AGE** (A4) si
Cypher devient un besoin utilisateur.

**Pourquoi ce choix (comparaison Palantir / ArgonOS, 2026-10-10)** :
l'ontologie de Palantir Foundry n'est pas une base graphe — objets
adossés à des tables, liens par clés, parcours bornés (« Search
Around »), exploration visuelle en surcouche. C'est notre modèle.
ArgonOS (ChapsVision) vise le renseignement, où le graphe de
connaissances est central (entités, événements, relations) ; chez nous
ce besoin ne concerne que la persona analyste média, et c'est le
déclencheur prévu pour AGE. L'écart réel avec ces plateformes n'est pas
le moteur de graphe mais les actions (D183), les droits fins (D188) et
l'intégration de sources.

Toutes les requêtes graphe passent par **une seule interface** (trait
Rust dans `pnex-core`) ; implémentation 1 = jointures + CTE + `petgraph`,
pure Rust testable sans base. L'API D185
reste bornée à 4 sauts ; l'illimité est réservé aux usages internes.

**PostgreSQL 18 maintenu** (tranché 2026-10-10, après un passage
éphémère en 19beta4) : la seule raison de PG 19 était SQL/PGQ, absent.
Passage à 19 quand il sera en GA **et** que PostGIS et pgvector le
suivront sur arm64 (la base de référence s'y applique déjà sans
changement, testé sur 19beta4).

### A4. Apache AGE : réserve, pas départ

- Extension PostgreSQL (Apache-2.0) : openCypher, chemins de longueur
  variable. Cible éventuelle : **projection en lecture alimentée par
  triggers**, Postgres relationnel restant la source de vérité ; feature
  optionnelle (2ᵉ implémentation de l'interface graphe), PNEX
  fonctionne sans.
- **Pas dans la 0.2.0.** Déclencheur d'adoption : parcours profonds
  (5+ sauts, plus courts chemins) sur des millions d'objets en temps
  interactif, devenus un vrai goulot.
- Pas AGE seul : perte des jointures avec les tables Loco, `agtype` sans
  schéma (ni FK ni `NOT NULL`), temporel verbeux en Cypher, ni SeaORM ni
  migrations Loco, retard possible sur les majeures de Postgres, rare en
  Postgres managé.
- Coût de la projection : stockage négligeable (~1–2 Go pour 1 M objets
  + 1 M liens, estimation), écriture rare ; le vrai coût est la
  cohérence de deux modèles. Écrire une extension Cypher sur les tables
  existantes : écarté (SQL/PGQ le fera nativement).

### A5. Alternatives écartées

| Option | Raison |
|---|---|
| Base graphe dédiée (Neo4j…) | Brique en plus, synchronisation, contraire à l'objectif Pi / 2 briques de stockage |
| SurrealDB | Licence BSL (incompatible MIT et fondation), maturité, réécriture complète de la couche données |
| XTDB | Bitemporel natif, mais JVM : trop lourd pour le Pi |
| AGE seul | Voir A4 |

### A6. Positionnement

- 0.2.0 : « **plateforme open source de données industrielles et IoT,
  pilotée par une ontologie** » (du firmware au jumeau numérique).
- Pas « alternative open source à Palantir / ArgonOS » tant que
  manquent : intégration massive de sources, gouvernance de niveau
  défense (classification, permissions ligne/colonne, lignage, audit),
  actions / writeback, apps par des non-devs, déploiement déconnecté.
- Plus tard (actions + gouvernance + connecteurs) : « plateforme
  opérationnelle ontologique open source ».

### A7. Questions ouvertes

- Types libres par org ou base fixe étendue : **déjà répondu par D176**
  (types système + types d'org en données + packs) — à reconfirmer.
- Introduction du **bitemporel** (temps système pour l'audit) : plus
  tard, sans casser le modèle (O2 `object_changes` couvre l'historique
  en attendant).

## 10. Mise en œuvre (2026-10-10)

L0 à L5 livrés le 2026-10-10 (commits fbf5f952 → e0298159, poussés sur
`main`). Ce qui suit consigne les choix d'implémentation pris en route,
sans revenir sur les décisions D176–D191.

### 10.1 Identité tenue par la base

- Une ligne `objects` par objet, système ou d'org (annexe A2). Pour les
  types système, l'identité est écrite par des **triggers PostgreSQL**
  (`pnex_object_identity`, `pnex_object_close`) : toute écriture native,
  seeds et chemins chauds compris, reçoit son identité dans la même
  transaction, sans code applicatif. Le trigger de mise à jour ne porte que
  sur la colonne de titre (`UPDATE OF`) : les écritures de présence ou
  d'état ne le déclenchent jamais.
- La table native porte `object_id` (FK, unique) ; `objects.native_id` est
  la clé native stringifiée (l'id de l'objet lui-même pour un type d'org).
  `resource_edges` reçoit ses deux identités par trigger à l'insertion.
- Supprimer une ligne native **ferme** l'identité (`valid_to`) ; la purge
  D42 (`purge_for`) **ferme** les liens au lieu de les supprimer.
- Les types système restent du code (`pnex_core::ontology`) ; les types
  d'org sont des données (`object_types` + `object_type_versions`
  append-only). Une ligne d'org sous une clé système = **extension** du
  type système (propriétés seulement, D176).
- Migrations : `m20261011_000002_ontology` (identité, types, liens
  temporels, packs, reprise 0.1 → 0.2) et `m20261011_000010_placed_at_links`
  (D191 : `device_placements` reste l'index matérialisé, un trigger tient
  les liens `placed_at` ; déplacer un device ferme puis rouvre).

### 10.2 Propriétés (D178)

Format maison sérialisé (`PropertyDef`, `kind` à plat) : `text`, `number`
(unité, bornes), `bool`, `date`, `date_time`, `enum`, `geo_point`, `url`
(http(s) seulement, R11), `ref`, `series`, `events`. Erreurs = jetons de
champ (`required`, `max_length:N`, `invalid`, `unknown`, `duplicate`).
`PropertyDef` se désérialise à la main via `Value` (piège
`arbitrary_precision` des enums taggés). `TypeSet` = `"*"` ou une liste de
clés, lisible en YAML.

### 10.3 Temps (D181, D182)

- Lien système `measures` (device → tout type), attributs `{metric,
  property}` ; une propriété `series` n'a qu'un capteur ouvert à la fois
  (409 `ontology-link-cardinality`). La série d'un objet est recomposée
  segment par segment, chaque segment découpé à la validité de son lien.
- `as_of` : les liens sont filtrés sur `valid_from`/`valid_to` ; les
  objets seulement sur leur fermeture (`valid_from` d'un objet = date
  d'enregistrement, pas date du monde réel).
- Plages : `scope_kind = object` (D182) ; fenêtres 7 j et 30 j ajoutées
  aux préréglages.

### 10.4 Requête, graphe, UI (D185–D188)

- `POST /api/v1/ontology/query` : une requête SQL paramétrée (CTE par
  saut, ≤ 4), clés de propriété contrôlées par jeu de caractères, valeurs
  toujours liées ; filtres `@>` (GIN) pour l'égalité.
- Graphe : CTE récursive bornée (5 000 liens, 3 s via `SET LOCAL
  statement_timeout`) puis `petgraph` dans `pnex_core::ontology::graph`
  (plus court chemin, impact en cascade, cycles). Le trait de stockage
  vit côté backend (asynchrone) ; les algorithmes, purs, dans `pnex-core`.
- UI : `/ontology` (objets, schéma, packs), `/ontology/object/:id`
  (propriétés, liens à date / historique, graphe radial SVG, temps,
  provenance), `/ontology/types/:type_key` (éditeur). Devices, médias et
  POI gagnent un bouton « Page objet ».
- Dashboards de type (D187) : `layout.object_type` + `source.object_property`,
  résolus au capteur courant (`/objects/{id}/bindings`) avant le pipeline
  `series-batch` inchangé. Raccourci : l'aperçu POI (`DashboardLive`) n'a
  pas de sélecteur d'objet.
- D188 : `write_role` par type (membre / admin / propriétaire) ; le schéma
  (types, types de liens, packs, import) exige un admin d'org.

### 10.5 Provenance, packs, assistant (D184, D189, D190)

- `source_ref` : `manual:<user>`, `assistant:<user>`, `device_placements` ;
  journal O2 `object_changes` (écriture en file, lecture sans
  provisionner), onglet Provenance.
- Packs : YAML embarqués (`crates/pnex-backend/packs/`, pack
  `maintenance`) ; installation = types créés puis versionnés, refus si une
  clé appartient à l'org hors pack ; export / import YAML du schéma d'org.
- Assistant : schéma de l'org injecté dans le prompt système et outil
  `describe_ontology` ; `query_ontology`, `get_object`, `create_object`,
  `update_object`, `open_link`, `close_link`, `save_object_type` via les
  services du contrôleur ; jamais d'archivage ni de suppression ; fiche
  `assistant-kb/ontology.md`.

### 10.6 Reste ouvert

- **D183 actions** : livrées 2026-10-13 (voir l'encadré sous D183). Restent
  l'éditeur d'actions dans l'éditeur de type, les préconditions, le retour
  de résultat par le flow et un plafond de débit par objet.
- Étiquetage d'ingestion `object=<kind>/<id>` (D181, dernier point) : non
  fait ; la liaison `measures` couvre le besoin.
- Mesures de performance sur 100 k objets (§8) : non faites.
- Deep-link de la trace assistant vers l'objet écrit : non fait (la trace
  nomme l'objet).
- `/api/v1/resources/*` reste servi tel quel (pas encore marqué
  déprécié), `CONTRACT` inchangé : les routes `/ontology/*` sont
  additives.

## Annexe B — Calendrier GMAO : étude de l'existant (pré-PRD, 2026-10-13)

> Statut : brouillon v0.1, phase de recherche. Aucun code, aucun numéro
> D : ils seront attribués au PRD « Calendrier GMAO » que cette étude
> alimente (roadmap P2.20, décision #26). La GMAO est un **pack** (D190) :
> le noyau ne gagne que ce qui sert aussi hors maintenance (récurrence,
> vue calendrier des plages).

### B1. Objectif

Avant de concevoir le calendrier de la GMAO, étudier une sélection de
projets open source sur quatre plans, et en tirer des **constats**, des
**patterns** et des **décisions** en langage naturel, réécrits ensuite
from scratch en Rust (Loco, Dioxus, `pnex-core`) :

1. **Modèle métier** : plan de maintenance, déclencheurs, occurrences,
   ordres de travail (OT), ressources.
2. **UX** : vues, interactions, densité d'information, conflits.
3. **Moteur** : récurrence, matérialisation, interop iCal/CalDAV.
4. **Optimisation** : affectation des équipes, contraintes.

### B2. Règles d'étude (clean-room)

PNEX est sous MIT ; une partie du corpus est sous copyleft
(GPL/AGPL/LGPL).

| Règle | Détail |
|---|---|
| Aucun extrait de code dans les notes | Ni copier-coller, ni pseudo-code calqué ligne à ligne sur une source copyleft. |
| Comportements, pas implémentations | « La date suivante est recalculée depuis la clôture de l'OT » ✅ ; « la fonction X appelle Y puis Z » ❌ |
| Schéma au niveau concept | Entités, relations, cardinalités, états ; jamais les noms de tables ou de colonnes. |
| Captures = usage interne | Elles servent à l'analyse UX et ne sont **pas versionnées** (`shots/` ignoré par git). |
| Séparation | Notes dans `research/gmao-calendar/`, hors du code ; le code s'écrit depuis le PRD, pas depuis les notes brutes. |
| Projets permissifs aussi | On réécrit par défaut. Seules des crates Rust s'utilisent telles quelles, à licence MIT/BSD/Apache vérifiée sur le `LICENSE` réel (règle des dépendances). |

### B3. Corpus

**GMAO — modèle métier**

| Projet | Licence | Ce qu'on cherche |
|---|---|---|
| Atlas CMMS (grashjs/cmms) | AGPLv3 (+ commerciale ; vérifié 2026-10-13) | Préventif **calendrier vs compteur**, génération automatique d'OT, règles d'automatisation, vue calendrier, portail de demandes, app terrain |
| Odoo Maintenance (Community) | LGPLv3 | Équipement → périodicité → demande ; calendrier/kanban des demandes ; équipes ; durée prévue vs réelle |
| ERPNext — Maintenance Schedule / Visit | GPLv3 | Échéancier généré depuis une périodicité ; plan vs visite réalisée |
| openMAINT | AGPL (numéro de version à confirmer) ; mobile propriétaire | Maintenance patrimoniale, lien bâtiments/localisations (POI, carte) |

**UI calendrier — vues et interactions**

| Projet | Licence | Ce qu'on cherche |
|---|---|---|
| vkurko/calendar (EventCalendar) | MIT | Vues **ressource et timeline** gratuites, drag & drop, resize, sélection de créneau, chevauchements, performances, architecture des vues |
| FullCalendar | MIT (core) / commercial (premium) | Référence d'interaction : sélection, heures ouvrées, événements de fond ; *resource timeline* observée sur la démo seulement, jamais dans le code premium |
| Frappe Gantt | MIT | Gantt léger pour les arrêts techniques, dépendances, glisser pour replanifier |

**Moteur — récurrence et interop**

| Projet | Licence | Usage |
|---|---|---|
| `rrule` (rust-rrule) | MIT OR Apache-2.0 (vérifié) | Dépendance candidate de `pnex-core` : wasm32 ? bornes anti-abus, fuseaux, `after`/`before` |
| `icalendar` (crate) | MIT/Apache-2.0 (vérifié) | Dépendance candidate pour l'export `.ics` |
| RustiCal | AGPL-3.0 (vérifié) : étude seulement | Étude seulement : faisabilité d'un CalDAV exposé plus tard |

**Optimisation**

| Projet | Licence | Ce qu'on cherche |
|---|---|---|
| Timefold — quickstart *maintenance scheduling* | Apache-2.0 | **Catalogue de contraintes** (conflit d'équipe, compétence, fenêtre au plus tôt / date limite, pénalité prématuré vs retard), modèle *TimeGrain*. Le solveur Java n'est pas repris. |

### B4. Méthode par projet

1. Lancer en local (docker compose ou démo officielle) avec un jeu
   réaliste : 3 sites, 20 équipements, 3 équipes, 10 plans préventifs
   mixtes (date + compteur).
2. Parcours UX chronométré sur les 6 scénarios (B6) : clics, frictions,
   ce qui est élégant.
3. Lecture du modèle de données au niveau concept (entités, états,
   transitions).
4. Lecture ciblée des sources pour un comportement précis ; seule la
   règle métier observée entre dans les notes.
5. Captures des vues clés dans `research/gmao-calendar/<projet>/shots/`.
6. Verdict : *à reprendre* / *à améliorer* / *à éviter*.

### B5. Gabarit de fiche

`research/gmao-calendar/<projet>.md` :

- **Identité** : version étudiée, date, licence, stack (contexte seul).
- **Modèle métier observé** : entités et relations ; cycle de vie d'un OT
  (états, transitions, qui déclenche) ; déclencheurs (date fixe, date
  flottante, compteur, condition) ; matérialisation des occurrences (à la
  volée, en base, horizon).
- **UX calendrier** : vues et usage de chacune ; interactions (drag,
  resize, création, replanification, multi-sélection) ; conflits,
  surcharge, absences ; filtres (site, équipe, équipement, criticité) ;
  mobile/terrain.
- **Points forts** (à reprendre), **faiblesses** (à éviter), **idées pour
  PNEX**, **verdict global**.

### B6. Scénarios communs

| # | Scénario | Ce qu'on observe |
|---|---|---|
| S1 | Plan « graissage tous les 3 mois » sur une pompe | Saisie de la récurrence, aperçu des prochaines dates |
| S2 | Plan « vidange toutes les 500 h de marche » | Déclencheur compteur, source, seuil, prévisionnel |
| S3 | Décaler une occurrence d'une semaine sans toucher la règle | Exception vs modification de série |
| S4 | Clôturer un OT en retard de 10 jours | Échéance suivante fixe ou flottante ? paramétrable ? |
| S5 | Charge d'une équipe sur 4 semaines et réaffectation | Vue ressource/timeline, détection de conflit |
| S6 | Arrêt technique de 3 jours avec 8 OT dépendants | Gantt, dépendances, fenêtre d'arrêt |

### B7. Grille comparative

`research/gmao-calendar/matrice.md` : note 0–3 par projet et par critère,
avec un commentaire court. Critères : expressivité de la récurrence
(RRULE complète, exceptions, fuseaux) ; déclencheurs compteur/condition ;
mode fixe vs flottant ; matérialisation et horizon ; vue mois/semaine ;
vue ressource/timeline (technicien, équipe, équipement) ; Gantt/arrêts ;
drag & drop et replanification ; conflits et surcharge ; filtres et
densité ; mobile/terrain ; interop (`.ics`, CalDAV) ; performance perçue
sur un gros jeu.

### B8. Hypothèses de modèle, ancrées sur l'existant

À confirmer ou invalider par l'étude. Règle : **réutiliser les briques
livrées**, ne rien redéfinir.

| Hypothèse | Brique PNEX existante |
|---|---|
| Un **plan de maintenance** est rattaché à un objet : device, équipement ou POI | identité universelle (annexe A2) : un device est déjà un objet ; **un plan sur un device seul marche sans aucun type utilisateur** (la GMAO n'impose pas l'ontologie) |
| Déclencheur `rrule` (date) | nouveau : types et validation de la récurrence dans `pnex-core`, compilés en wasm32, comme `check_values` (même code pour l'aperçu dans l'éditeur Dioxus et pour le backend) |
| Déclencheur `meter` : seuil sur une mesure O2 (heures de marche, cycles, vibrations) | propriété `series` (D178/D181) ; cas M5 de `use-cases.md` (cumul calculé par flow) |
| Déclencheur `condition` : alerte issue d'un flow | flows + notifications (D86) ; nœuds anomalie/forecast (M2–M4) |
| Mode de recalcul `fixed` (depuis la date prévue) ou `floating` (depuis la clôture) | nouveau (scénario S4) |
| **Occurrences** matérialisées en Postgres sur un horizon glissant (ex. 90 j) par un worker Loco ; exceptions (déplacée, annulée) portées par l'occurrence, pas par la règle | une occurrence est une **plage** `time_ranges` à portée objet (D182) ; à trancher : plage seule ou table dédiée |
| **OT** généré depuis une occurrence : automatique, à J-n ou manuel | l'OT est la **procédure instanciée** des Pages (`pages.md` P9), jamais un nouvel objet « bon de travail » |
| Ce qui doit agir au début d'une occurrence (mise en maintenance, consignation) | **action** déclarée (D183, ex. `start_maintenance`), exécutée par un flow déployé, jamais par le calendrier (D123) |
| **Ressources** : technicien, équipe, compétence, équipement immobilisé | types et liens du pack Maintenance (D179) ; personnes = utilisateurs de l'org ou objets du pack, à trancher |
| Assistant IA | il propose un plan ou un décalage (pré-rempli), jamais d'écriture silencieuse (D142–D145) |

### B9. Livrables de la phase

1. `research/gmao-calendar/<projet>.md` : une fiche par projet (B5).
2. `research/gmao-calendar/matrice.md` : la grille (B7).
3. `research/gmao-calendar/patterns.md` : le best-of en règles métier et
   patterns UX, sans référence au code.
4. `research/gmao-calendar/modele-cible.md` : le modèle de domaine
   proposé (B8 tranché) et les questions ouvertes.
5. Maquettes des vues retenues aux jetons PNEX (`design-system.md`),
   avant tout code (« UI design d'abord »).

### B10. Hors périmètre de la phase

- Toute implémentation Rust.
- Stock, pièces détachées, achats, coûts (notés seulement s'ils
  touchent le calendrier).
- Le solveur d'optimisation (seul le catalogue de contraintes est
  collecté).
- Le serveur CalDAV (faisabilité seulement).

### B11. Questions ouvertes

1. Le compteur produit-il une **date prévisionnelle** (extrapolation de
   la tendance) affichée au calendrier ? Avec quel modèle (le nœud
   forecast existant ?) ?
2. Granularité de l'horizon de matérialisation, configurable par org ?
3. **Calendriers ouvrés** (fériés, astreintes, horaires d'équipe) dès la
   v1 ?
4. Représentation d'un OT **correctif** (non planifié) pour voir la
   charge réelle ?
5. Mode hors ligne pour le terrain en v1 ?
6. Le calendrier est-il une vue **générique des plages** (noyau, utile
   au média et aux dossiers) dont la GMAO n'est qu'un usage, ou une vue
   du pack ? (lien `pages.md` §10 : vues calendrier/Gantt des
   collections, repoussées)

### B12. Critères de fin de phase

> État 2026-10-13 : étude **documentaire** faite (docs, démos, tickets
> publics ; aucune instance lancée), livrables dans `research/gmao-calendar/`.
> Constats clés : aucun projet ne couvre à la fois modèle, vue de charge
> et Gantt ; le compteur est partout manuel (avantage PNEX : télémétrie) ;
> aucune interop calendrier ; FullCalendar premium exclu (licence) →
> EventCalendar (MIT) embarqué + Gantt Dioxus natif proposés ; `rrule`
> MIT/Apache mais maintenance ralentie et wasm32 non prouvé.

- [ ] Les 6 scénarios joués sur au moins Atlas, Odoo et EventCalendar.
- [x] Une fiche par projet du corpus (B3) — étude documentaire
  2026-10-13, scénarios « à jouer » encore ouverts.
- [x] Matrice comparative remplie (version documentaire).
- [x] `patterns.md` : 18 patterns et 9 anti-patterns.
- [ ] `modele-cible.md` relu, hypothèses B8 tranchées (proposition
  écrite 2026-10-13, relecture à faire).
- [ ] Relecture finale : aucun extrait de code tiers dans `research/`.

### B13. Premier lot de code (2026-10-13, branche `feat/gmao-plans`, 21f68103)

Lancé sur l'accord de l'user, avant la relecture complète de
`modele-cible.md`. Ce qui est fait, et en quoi cela s'écarte de la
proposition :

- **Noyau** :
  - type de propriété **`recurrence`** dans `pnex-core`
    (`ontology/recurrence.rs`) : début, tous les N jours, semaines, mois ou
    ans, fin facultative, `floating`, durée en jours ;
  - énumération bornée (1000 dates au plus) et fins de mois sans dérive.

  **Écart** : pas de RRULE ni de crate `rrule`. Une période simple
  couvre S1, S2 en date et S4. Il faudra passer à `rrule` quand un plan
  demandera BYDAY ou une heure. La crate `chrono` (sans horloge) entre
  dans `pnex-core`.
- **Matérialiseur** (`services/ontology/recurrence.rs`) :
  - fonctionnement :
    - horizon de 90 jours ;
    - `external_id = rec:<propriété>:<date>`, origine `grid` ;
    - recalcul après chaque création, modification ou archivage d'objet, et
      après la modification d'une occurrence ;
    - balayage toutes les 6 h, sur un seul pod (D106).
  - **ce qu'un utilisateur a touché n'est jamais réécrit** (réalisée,
    statut posé, ou déplacée). Les occurrences futures intactes sortent de
    la série quand la règle change ou que l'objet est archivé.
  - **Écart** : les occurrences portent sur **l'objet qui tient la
    récurrence** (le plan), pas sur la cible. La cible se rejoint par le
    lien `maintains`, et le mécanisme reste générique : toute propriété
    `recurrence` de tout type produit ses plages.
- **Pack Maintenance 1.2.0** : type `maintenance_plan` (`schedule`,
  `instructions` ; `immobilizes` retiré tant qu'il ne fait rien) et lien `maintains` vers
  n'importe quel type, device compris.
- **UI** :
  - saisie de la récurrence avec aperçu des 5 prochaines dates (même code
    que le serveur) ;
  - page **`/calendar`** : grille du mois native Dioxus, états planifiée,
    en retard (calculé), réalisée, annulée ;
  - dialogue d'occurrence : marquer réalisée, annuler, déplacer ;
    l'occurrence seule change, jamais le plan.
  
  **Écart** : ni EventCalendar ni vue de charge pour l'instant (pas encore
  d'équipes).
- **Sécurité** : une plage d'objet (création, modification, suppression,
  import) exige désormais le `write_role` du type de l'objet (D188), comme
  l'objet lui-même. Cet écart préexistait (D182) ; test viewer/member →
  403 `ontology-write-forbidden`. Le nœud `range_upsert` (flow déployé
  par un humain) n'est pas concerné.
- **Assistant** :
  - fiche `assistant-kb/calendar.md` ;
  - les plans se créent par `create_object`, dont la description donne
    la forme d'une récurrence ;
  - **`list_calendar`** (lecture, avec l'état calculé) ;
  - **`propose_occurrence_change`** : réalisée, déplacer ou annuler. Rien
    n'est écrit ; mêmes règles que l'API (`scope_writable`, partagé avec
    le contrôleur) ; le bouton « Vérifier » sous la réponse ouvre le
    dialogue du calendrier pré-rempli.
- **Second passage (même jour)** :
  - **état d'occurrence partagé** dans `pnex-core`
    (`time_range::occurrence_state`) : planifiée, à préparer
    (`lead_days`, devenu un champ de la récurrence), en retard, réalisée,
    annulée ;
  - les occurrences intactes suivent le titre et le délai du plan ;
  - **clôture** avec date et temps passé ;
  - `GET /time-ranges/{id}` ;
  - **carte « Occurrences »** dans l'onglet Temps de l'objet qui porte la
    récurrence **et de tout objet lié à lui** (la pompe montre son
    prochain graissage) : générique, sans connaître le lien `maintains`.
- **Reste à faire** :
  - **côté terrain** : compteur (S2 en heures de marche), OT = procédure
    (Pages L6), équipes et vue de charge, Gantt d'arrêt, flux `.ics` ;
  - **côté occurrences** : proposer l'action D183 (`start_maintenance`) à
    l'ouverture d'une occurrence ; jours en UTC.
