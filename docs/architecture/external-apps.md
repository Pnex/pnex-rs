# Applications sur PneX : SDK d'applications (D432–D441)

> **Statut : questions Q-A1 à Q-A7 tranchées par Shan le 2026-10-25
> (§13)** : les défauts, sauf Q-A6 (D438 réécrite). La garde D441 est
> livrée ; les lots suivants attendent le feu vert de Shan pour le code.
>
> Demande de Shan, relayée par la session `pnex-quali-watch` : un socle
> pour que n'importe quelle application métier se monte vite au-dessus de
> l'ontologie PneX, à la manière de l'Ontology SDK de Foundry. Vigie
> (veille.pnex.io) est la première application ; rien ici ne lui est
> propre.
>
> Liens : `ontology.md` (D176–D190, en particulier D183 actions, D185
> requêtes, D190 packs), `iam.md` (comptes de service), `media.md`,
> `network-check.md` (D408 preuves serveur, D421 documents d'une page),
> `api-contract.md` (D238–D245), `mcp.md`.

## 1. Principe : PneX est une plateforme de données générique (D432)

### D432 — Le moteur ne connaît aucun métier ; les applications passent par le SDK

PneX est une plateforme de données opérationnelle générique, de la
catégorie de Palantir Foundry : capter des sources, les rattacher à des
objets typés, les relier dans le temps, agir, et dire d'où vient chaque
fait (`ontology.md` §2).

Le métier vit à deux endroits seulement :

- dans les **packs** (D190) : types, liens, réglages, fonctions, flows et
  règles d'un domaine ;
- dans les **applications** : des programmes hors de PneX (Vigie en est
  une) qui lisent et écrivent l'ontologie par le **SDK d'applications**
  décrit ici.

Le SDK offre huit briques, toutes génériques :

| Brique | Rôle | Décision |
|---|---|---|
| Identité d'application | L'application déclarée dans l'org, son compte de service, ses droits et son périmètre | D433 |
| Ensembles d'objets | Requêtes déclaratives nommées sur l'ontologie | D434 |
| Abonnements | Flux des changements, avec un curseur reprenable, puis du push | D435 |
| Médias | Récupération des pièces, et dépôt par une action | D436 |
| Preuves et provenance | D'où vient chaque donnée ; les preuves de tout objet | D437 |
| Client et exemples | Le client OpenAPI générique (`task sdk`) et des exemples par cas d'usage | D438 |
| Miroir hors ligne | Copie locale SQLite des objets, liens et médias | D439 |
| Actions | Écritures typées, validées par le moteur, journalisées | D440 |

**Règle vérifiable** (garde bloquante, D441) : aucun nom de type, de
lien ou de pack d'un pack livré (`crates/pnex-backend/packs/*.yaml`)
n'apparaît dans le code du moteur ni dans le SDK, hors tests. Une
exception porte sa raison dans la garde, comme pour `lexicon_guard`.

Conséquences :

- une fonctionnalité demandée par une application est conçue pour toutes
  les applications, avec un exemple sur une ontologie inventée ;
- une application ne fait jamais d'écriture libre : elle appelle une
  action (D440) ;
- les exemples et les tests du SDK utilisent une ontologie inventée
  (`article`, `auteur`, lien `ecrit_par`), jamais un pack livré.

## 2. État de l'existant (relevé du 2026-10-25)

| Brique | Existe | Manque |
|---|---|---|
| Identité | Comptes de service (clé `pnex_sk_…`, Bearer direct, rôle `viewer` par affectation IAM). Règle V1 : un compte de service ne détient que des permissions `*:read`. | Notion d'application, périmètre, droits d'action. |
| Ensembles d'objets | `POST /ontology/query` (D185) : filtres, `traverse` (4 sauts), `with_links`, `latest`, pagination 500. | Requête nommée et réutilisable, agrégats simples. |
| Changements | Journal O2 `object_changes` (D184), au mieux, par objet, sans curseur. `GET /ontology/objects/{id}/changes` (200 derniers). | Flux à l'échelle de l'org, monotone, sans perte. |
| Médias | `GET /media/{id}/content` et `/versions/{n}/content`, ETag fort (id de version), 304, `media:read` (Viewer). sha256 par version. Chaque média est un objet natif `media_asset`. | Rien côté moteur pour le lot 1. |
| Preuves | `web_proofs` + `site_pages` (D408) ; pièces = médias. Lien `prouve_par` de l'objet page vers les médias des pièces (`link_parts`), si le pack le configure. | Accès « preuves d'un objet » pour tout objet ; le filtre `page_id` des preuves porte sur `site_pages.id`, **pas** sur l'objet page de l'ontologie (§7). |
| Client | `task sdk` génère un client OpenAPI non commité ; `pnex-cli` est dynamique. | Client typé par les types de l'org. |
| Miroir | Vigie recopie tout toutes les 15 min dans SQLite (`doc(key, body)`). | Miroir générique et incrémental. |

## 3. Identité d'application (D433)

### D433 — Une application est un objet de l'org qui habille un compte de service

Une **application** est déclarée dans l'org (page Accès, onglet
« Applications »), par un admin. Elle porte :

- `key` (unique dans l'org), `name`, `description` ;
- son **compte de service** (existant, `iam.md`) : l'application
  s'authentifie avec sa clé, comme aujourd'hui ;
- son **périmètre** : types d'objets, types de liens, genres de médias
  (`kinds`) et taille maximale des pièces qu'elle peut recopier ;
- ses **ensembles d'objets** nommés (D434) ;
- ses **actions permises** (D440, lot 2).

Le périmètre est **appliqué par le serveur** au flux (D435) et aux
ensembles d'objets (D434) : une application ne reçoit que ce qu'elle a
déclaré, même si son rôle IAM lui en permettrait plus (Q-A1, Shan). Les routes
génériques (`/ontology/query`, `/media/{id}/content`) restent gouvernées
par le rôle IAM du compte, comme pour tout appelant.

Livré (lot A2) :

- table `apps (id, org_id, key, name, description, service_account_id
  unique, scope jsonb, version, created_by, created_at, updated_at)`,
  migration 000055 ; les ensembles d'objets s'y ajoutent au lot A3, les
  actions au lot 2 ;
- `GET/POST /api/v1/apps`, `GET/PUT/DELETE /api/v1/apps/{id}` (PUT sur
  `expected_version`, 409 `app-version-conflict`), `GET /api/v1/apps/me`
  (l'application du compte de service appelant, 404 `app-not-found`
  sinon) ; permissions `apps:read` (Viewer), `apps:write` et
  `apps:delete` (Admin) ; MCP : lecture exposée, création et
  modification exclues (motif accès), suppression exclue (destructif) ;
- contrôles : clé unique, compte de service de l'org et d'une seule
  application, types et types de liens existants, genres de médias de
  la médiathèque, taille de 1 octet à 2 Gio ;
- le flux (D435) restreint les types et types de liens à ceux du
  périmètre quand l'appelant est le compte d'une application ;
- onglet « Applications » de la page Accès (déclaration, périmètre par
  cases à cocher), fiche `assistant-kb/access.md`, mot « Application »
  au lexique et règle de la garde.

Un pack peut **livrer** une déclaration d'application (`apps:` dans le
YAML), que l'admin installe et relie à un compte de service : c'est la
voie pour qu'une application métier arrive « prête » avec son pack, sans
que le moteur la connaisse.

## 4. Ensembles d'objets (D434)

### D434 — Un ensemble d'objets est une requête D185 nommée

Un ensemble d'objets est une `OntologyQuery` (D185) enregistrée sous un
nom, dans la déclaration de l'application (ou de l'org, plus tard). Rien
de neuf côté moteur de requête : on reprend `/ontology/query`, `traverse`
et `with_links`.

- `GET /api/v1/apps/me/object-sets/{name}?limit&offset` exécute
  l'ensemble, dans le périmètre de l'application (les types et liens hors
  périmètre sont refusés à l'enregistrement, `app-scope-violation`).
- Paramètres : un ensemble peut déclarer des paramètres nommés
  (`$code`), substitués dans les valeurs des filtres seulement, jamais
  dans les clés (pas d'injection de structure).
- Agrégats simples (lot 2) : `count` et `group_by` une propriété, pour
  les compteurs d'écran ; le reste se calcule dans l'application.

## 5. Abonnements : le flux des changements (D435)

### D435 — Un journal des changements en base, à curseur monotone

Le journal O2 (D184) reste la provenance détaillée (avant/après), mais il
ne sert pas à synchroniser : il est au mieux (file bornée), sans curseur,
et sa rétention est celle d'O2. Le flux de synchronisation vit donc en
base.

**Table** `ontology_changes (seq bigint identity, org_id, txid xid8,
at, kind, id, type_key, action)` (migration 000054) :

- `kind` : `object` ou `link` ; `id` : uuid de l'objet, ou id du lien ;
  `type_key` : type de l'objet, ou type du lien ;
- `action` : `create`, `update`, `close` (objet archivé, lien fermé) ou
  `delete` (ligne supprimée) ;
- écrite par des **triggers d'instruction** (tables de transition) sur
  `objects` et `resource_edges` : toutes les voies d'écriture sont
  couvertes (API, flows, imports en masse, objets natifs dont les
  médias), et un import de 60 000 lignes n'en paie qu'un appel par
  instruction ; une mise à jour sans changement n'écrit rien ;
- index `(org_id, txid, seq)` et `(org_id, kind, id)`.

**Curseur sans trou** : un `seq` est pris avant le commit, donc ordonner
par `seq` sauterait une transaction longue qui valide après qu'un lecteur
a avancé. Le flux ordonne par **(transaction, seq)** et ne lit que les
transactions plus anciennes que toutes celles encore en cours
(`txid < pg_snapshot_xmin(pg_current_snapshot())`) : une transaction qui
valide tard a un identifiant plus grand que tout ce qui a déjà été
servi. Le curseur est `<txid>.<seq>` (`0` = le début). Prix : une
transaction longue (un import) retient le flux jusqu'à sa fin.

**Journal compacté** : seule la **dernière** ligne de chaque objet et de
chaque lien est gardée (tâche horaire, singleton). Un remplissage initial
écrit une ligne par objet et par lien existants. Conséquences :

- `since=0` est une **copie complète** (l'état courant de tout ce qui
  existe), sans route à part ;
- un lecteur en retard reçoit l'état courant, jamais les états
  intermédiaires qu'il a manqués ;
- les lignes `close` et `delete` partent après la **rétention** (30
  jours) ; leur position devient la marque de l'org
  (`ontology_change_marks`). Un `since` plus ancien que la marque répond
  410 `changes-cursor-expired` : l'application vide sa copie et repart de
  `0`.

**Routes** (`ontology:read`, comptes de service compris ; MCP exposé) :

- `GET /api/v1/ontology/changes?since=&limit=&types=&link_types=` :
  `{changes: [{cursor, at, kind, action, id, type_key, object?|link?}],
  next, more}`, du plus ancien au plus récent ; l'objet ou le lien
  courant est joint (`ObjectView`, `LinkView`, avec `valid_to` s'il est
  fermé) ; plusieurs changements d'un même objet dans la page sont
  repliés sur le dernier ; une page courte avance `next` jusqu'à la tête,
  par-delà les lignes que le filtre écarte ; `limit` 1 à 1000 (500) ;
- `GET /api/v1/ontology/changes/head` : la position la plus récente,
  pour qui a copié l'org autrement et veut suivre à partir de là ;
- pour une application, le filtre viendra de son périmètre (D433, lot
  A2).

**Push (lot 2)** : `GET /api/v1/ontology/changes/stream?since=` en SSE,
qui ne pousse que des curseurs (« il y a du neuf jusqu'à `seq` ») ;
l'application relit par la route paginée. Pas de websocket au lot 1.

## 6. Médias (D436)

### D436 — Les médias se recopient par leurs routes existantes

Un média est un objet natif `media_asset` : il entre dans le flux comme
tout objet, dès que son type et le lien qui le relie sont dans le
périmètre de l'application. Sa vue porte la version courante, son
`sha256`, sa taille et son type de contenu.

- Téléchargement : `GET /media/{id}/content` (ETag fort, 304), ou
  `/versions/{n}/content` pour une version précise ; `media:read`.
- Le miroir (D439) choisit par configuration ce qu'il télécharge (genres,
  taille maximale), vérifie le sha256, range par empreinte, reprend après
  coupure et garde le dernier fichier valide.
- Dépôt (lot 2) : une action `media.upload` (D440) crée un média et peut
  le relier à un objet du périmètre ; même liste blanche de formats que
  l'UI.

Aucun genre de média n'est codé en dur dans le SDK : la configuration de
l'application liste ceux qu'elle veut.

## 7. Preuves et provenance (D437)

### D437 — Toute donnée dit d'où elle vient ; les preuves d'un objet se lisent depuis lui

**Provenance** : chaque objet et chaque lien porte déjà `source_ref`
(`import:<id>@<run>`, `flow:<id>@<version>`, `user:<id>`, `pack <key>
<version>`). Le flux (D435) et les ensembles d'objets le rendent tel
quel ; le SDK l'expose en type (`Provenance::{Import, Flow, User, Pack,
Other}`).

**Constat sur l'existant** : le filtre `page_id` de
`GET /extension/proofs` porte sur `site_pages.id`, une ligne du registre
des pages, et non sur l'objet page de l'ontologie (`site_pages.object_id`).
Un écart qui pointe `constate_dans` vers l'objet page ne permet donc pas
de retrouver ses preuves par cette route. Le seul chemin aujourd'hui est
`prouve_par`, de l'objet page vers les médias des pièces, posé au
scellement si le pack le configure.

**Décision** : un type de lien peut se déclarer **de preuve**
(`evidence: true` dans son schéma, posé par le pack). Deux effets
génériques :

1. `GET /api/v1/ontology/objects/{id}/evidence` rend les preuves de
   **tout** objet : les médias atteints par des liens de preuve, à deux
   sauts au plus (l'écart → `constate_dans` → la page → `prouve_par` →
   les pièces), avec pour chacun `captured_at`, `sha256`, genre, url
   d'origine, et le scellement de la preuve web quand il existe
   (`seq`, `manifest_sha256`, `prev_sha256`). `ontology:read` +
   `media:read`.
2. **Lien direct** : quand une preuve web est scellée, le moteur relie
   aussi aux pièces les objets qui pointent la page par un lien de preuve
   sans en avoir encore (un écart détecté avant la capture). Ainsi un
   écart porte le lien vers la preuve sur laquelle il repose, à la date
   de sa détection, et non vers les captures suivantes de la page (Q-A3,
   Shan : pas d'objet natif pour la preuve).

Dans le pack `quali_watch` (seule partie propre au métier) :
`constate_dans` et `prouve_par` passent `evidence: true`. Les règles
n'ont rien d'autre à faire.

## 8. Client et exemples (D438)

### D438 — Le SDK est le client OpenAPI générique, plus des exemples par cas d'usage

Décision de Shan (Q-A6) : pas de client généré, ni par type, ni par
application, ni par pack ; maintenir plusieurs petits clients coûte trop.
Le seul client est celui que `task sdk` génère déjà du contrat public
(`docs/contracts/openapi.json`) : il couvre toutes les routes, dont
celles de ce document. Les objets s'y lisent en JSON (propriétés par
clé), comme dans le miroir.

Il est complété par `pnex-mirror` (D439) et par des **exemples
documentés, par cas d'usage**, dans `crates/pnex-mirror/examples/`,
chacun sur l'ontologie inventée (`article`, `auteur`, `ecrit_par`) et
lancé par un test contre un faux serveur :

| Exemple | Ce qu'il montre |
|---|---|
| `offline_copy` | Recopier le périmètre d'une application hors ligne, et servir ses médias |
| `follow_changes` | Suivre le flux des changements et réagir à chaque objet (projection) |
| `object_evidence` | Afficher les preuves d'un objet, avec ses pièces vérifiées |
| `object_set` | Lire un ensemble d'objets nommé, avec ses paramètres |
| `upload_by_action` | Déposer un fichier par une action (lot 2, avec D440) |

Chaque exemple a son en-tête : le besoin, les routes appelées, les
permissions et le périmètre qu'il demande.

## 9. Miroir hors ligne (D439)

### D439 — `pnex-mirror` : la copie locale générique

Crate `crates/pnex-mirror` du workspace, importée par git par les
applications. Elle ne dépend ni du backend ni de `pnex-core` (Q-A5) :
ses types de contrat sont les siens, vérifiés contre
`docs/contracts/openapi.json` par un test.

Contenu :

- **client** du compte de service (`reqwest` rustls, Bearer, `X-Org-Id`,
  reprises avec attente croissante) ;
- **consommation du flux** (D435) : boucle `since → next`, curseur stocké
  dans la même transaction SQLite que les lignes, donc reprenable ; 410 →
  copie complète puis reprise ;
- **SQLite** (`rusqlite`, `bundled`) : tables `objects(id, type_key,
  version, title, properties json, source_ref, valid_from, valid_to,
  updated_at)`, `links(id, link_type, source_id, target_id, attributes
  json, source_ref, valid_from, valid_to)`, `media(asset_id, version,
  sha256, kind, content_type, size, fetched_at)`, `cursor` ; propriétés
  en JSON, sans projection ;
- **médias** : téléchargement en tâche de fond, filtres par genre et
  taille, sha256 vérifié, fichiers rangés par empreinte
  (`<dir>/ab/cd/<sha256>`, sans doublon), ETag et 304, dernier fichier
  valide gardé si le moteur tombe ; `Mirror::media(asset_id) -> Option<
  (Path, content_type)>` pour que l'application serve ses utilisateurs
  sans repasser par PneX ;
- **projection** : un trait `Projection { fn on_object, fn on_link, fn
  on_media }` appelé dans la transaction de chaque page appliquée, pour
  que l'application tienne ses propres vues ; ou des requêtes SQL
  directes sur le miroir ;
- les **exemples par cas d'usage** (D438) et des tests sur une ontologie
  inventée (`article`, `auteur`, `ecrit_par`) avec un faux serveur local.

## 10. Actions (D440, lot 2)

### D440 — Une application écrit seulement par des actions déclarées

Les actions D183 (déclarées sur les types, exécutées par des flows,
tracées) sont la forme d'écriture des applications. S'y ajoutent des
actions de base, génériques : `object.create`, `object.update` (sur
`expected_version`), `link.open`, `link.close`, `media.upload`, chacune
bornée par le périmètre de l'application et par sa liste d'actions
permises.

- Route : `POST /api/v1/apps/me/actions/{action}` ; validation du moteur
  (schéma du type, unicité, cardinalité, 409 sur version), journal O2 et
  `source_ref = app:<key>`.
- La règle V1 « un compte de service ne fait que lire » est assouplie
  pour les seuls comptes d'application, et pour les seules actions
  déclarées (Q-A4). Aucune action physique ni de déploiement (D123,
  D144).

## 11. Garde de généricité (D441)

### D441 — Une garde refuse le métier dans le moteur

Test bloquant `crates/pnex-backend/tests/engine_genericity.rs` :

- lit les clés de pack, de type et de type de lien de tous les packs
  livrés ;
- parcourt le code non test (avant le premier `#[cfg(test)]`) des
  `crates/*/src` et de `crates/pnex-mirror` ;
- échoue sur toute chaîne littérale égale à l'une de ces clés, sauf
  exception listée avec sa raison (le catalogue des packs livrés,
  `services/ontology/packs.rs`, ne nomme que des fichiers et passe).
- Livrée en avance sur le reste (lot A0), puisqu'elle ne fait que
  constater.

Deux listes motivées, dans la garde :

- **mots du moteur** qu'un pack emploie aussi comme clé (`media`, `site`,
  `line`, `controls`, `about`, `before`, `porte`, `programme`) : admis
  partout ;
- **couplages existants, gelés** : fichier, clés, raison. Tout nouveau
  couplage fait échouer la garde ; chaque ligne est une dette.

**Constat du 2026-10-25** :

- `quali_watch` et `presence_web` : **aucune** clé dans le code non test
  du moteur.
- Couplages réels, à traiter à part (gelés dans `COUPLED`) :
  - les **dossiers d'enquête** (`services/ontology/cases/`,
    `pages/cases/`) sont construits sur les types du pack `investigation`
    (`person`, `organization`, `place`, `hypothesis`, `claim`, `piece`,
    `identifier`, `supports`, `contradicts`, `sourced_by`, `statement`,
    `party`, `owns`) ; la règle de statut des `claim` est dans
    `objects.rs` ;
  - des fonctions transverses écrivent des types ou liens
    d'`investigation` : profils des agents navigateur (`person`,
    `identifier`, `uses_identifier`), attributions de médias
    (`appears_in`), vidéos collectées (`published_by`), relecture média
    (`statement`, `said_by`), profils vocaux (`person`) ;
  - les chemins d'introduction de la cartographie de comptes (D329,
    `ontology/api.rs`) nomment des liens d'`investigation`,
    d'`entreprises` et de `prospection_b2b` (`knows`, `recommande`,
    `employs`, `dirige`, `member_of`).
  - Piste commune : un **rôle déclaré** par le pack (le type « personne »,
    le lien « appuie »…), que le moteur lit au lieu d'un nom, comme
    `evidence: true` (D437).
- Hors garde (pas une clé de pack) : le nœud `org_resolve` (nom → SIREN)
  et le mode d'import `datagouv` sont des connecteurs à des sources
  publiques françaises : génériques pour tout pack, mais propres à un
  pays ; à ranger comme connecteurs déclarés, comme la géo (Q-A7).
- Des modules de test dans `src` et un test du front incluent
  `quali_watch.yaml` : admis (tests), à migrer vers une ontologie
  inventée quand on y touche.

## 12. Lots

| Lot | Contenu | Tests |
|---|---|---|
| **A0** | D432 principe + D441 garde | `engine_genericity.rs` |
| **A1** | D435 : table, triggers, curseur, `/ontology/changes` (+ `head`), compaction | ordre et absence de trou avec deux transactions croisées ; import en masse ; objet natif ; 410 |
| **A2** | D433 : `apps` (CRUD, `apps/me`, périmètre appliqué au flux) | 403 sans permission ; périmètre filtrant |
| **A3** | D434 : ensembles d'objets nommés, paramètres | substitution des valeurs seulement ; périmètre |
| **A4** | D437 : `evidence: true`, `/objects/{id}/evidence`, lien direct au scellement ; pack `quali_watch` | écart → page → pièces sur une ontologie inventée |
| **A5** | D439 : `pnex-mirror` (client, flux, SQLite, médias, projection, exemple `offline_copy`) | faux serveur ; coupure et reprise ; sha256 faux refusé ; 304 |
| **A6** | D438 : exemples par cas d'usage (client OpenAPI + `pnex-mirror`) | chaque exemple lancé contre le faux serveur |
| **A7** | D421 : documents d'une page dans `site_crawl` (config `record.documents`), générique | serveur de test local |
| Lot 2 | D440 actions (et l'exemple `upload_by_action`), SSE, agrégats | — |

Vigie bascule après A5.

## 13. Questions tranchées (Shan, 2026-10-25)

Les défauts, sauf Q-A6 :

- **Q-A1 — Périmètre** : appliqué par le serveur au flux et aux ensembles
  d'objets ; les autres lectures restent gouvernées par le rôle IAM
  (D433).
- **Q-A2 — Rétention du flux** : 30 jours, puis copie complète (D435).
- **Q-A3 — Preuves** : liens `evidence` et lien direct au scellement ;
  pas d'objet natif pour la preuve (D437).
- **Q-A4 — Écritures des applications** : par actions déclarées
  seulement, au lot 2 (D440).
- **Q-A5 — Types du SDK** : propres à `pnex-mirror`, vérifiés contre le
  spec OpenAPI (D439).
- **Q-A6 — Client** : **pas de client généré** ; le client OpenAPI
  générique, des exemples par cas d'usage et `pnex-mirror` (D438).
- **Q-A7 — Connecteurs français** (`org_resolve`, `datagouv`) : à ranger
  en connecteurs déclarés, chantier séparé.
