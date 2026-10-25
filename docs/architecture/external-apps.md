# Applications sur PneX : SDK d'applications (D432–D441)

> **Statut : proposé (2026-10-25), questions Q-A1 à Q-A7 pour Shan (§12).**
> Pas de code avant l'accord de Shan, sauf la garde de D432, qui ne fait
> que constater l'état du moteur.
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
| Client généré | Client typé tiré des types de l'org | D438 |
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

Une **application** est déclarée dans l'org (page Organisation,
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
déclaré, même si son rôle IAM lui en permettrait plus (Q-A1). Les routes
génériques (`/ontology/query`, `/media/{id}/content`) restent gouvernées
par le rôle IAM du compte, comme pour tout appelant.

Stockage : table `apps (id uuid, org_id, key, name, description,
service_account_id, scope jsonb, object_sets jsonb, actions jsonb,
version, created_at, updated_at)`. CRUD sous `/api/v1/apps`, permission
`apps:read` (Viewer) et `apps:write` (Admin) ; contrat OpenAPI, IAM et
MCP au même commit (création et suppression exclues du MCP, motif
`accès`). L'application connaît sa propre déclaration par
`GET /api/v1/apps/me` (compte de service appelant).

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

**Table** `ontology_changes (seq bigint generated always as identity,
org_id, at timestamptz, txid xid8, kind text, id text, type_key text,
action text, version int, source_ref text)` :

- `kind` : `object` ou `link` ; `id` : uuid de l'objet, ou id du lien ;
- `action` : `create`, `update` ou `close` (objet archivé, lien fermé) ;
- écrite par des **triggers** `AFTER INSERT OR UPDATE` sur `objects` et
  `resource_edges` : toutes les voies d'écriture sont couvertes (API,
  flows, imports en masse, objets natifs dont les médias), sans qu'un
  service puisse l'oublier ;
- index `(org_id, seq)`.

**Curseur sans trou** : un `seq` est pris avant le commit, donc une
transaction longue peut rendre visible un `seq` plus petit après qu'un
lecteur a avancé. Le flux ne rend que les lignes dont la transaction est
plus ancienne que toutes celles encore en cours
(`txid < pg_snapshot_xmin(pg_current_snapshot())`) ; le curseur rendu est
le dernier `seq` livré. Un lecteur ne perd donc rien, au prix d'un
léger retard pendant une transaction longue (un import).

**Route** `GET /api/v1/ontology/changes?since=<seq>&limit=<n>` (et
`types=`, `link_types=` pour un appelant hors application) :

- réponse `{changes: [{seq, at, kind, action, id, type_key, version,
  source_ref, object?|link?}], next: <seq>, more: bool}` ; l'objet ou le
  lien courant est joint (`ObjectView`, `LinkView`), un objet fermé est
  rendu avec `valid_to` ;
- plusieurs changements d'un même objet dans la page sont repliés sur le
  dernier ;
- pour une application, le filtre vient de son périmètre (D433) ;
- `since` plus ancien que la rétention (30 jours par défaut, réglage
  plateforme) : 410 `changes-cursor-expired`, l'application refait une
  copie complète par ses ensembles d'objets puis repart du `next` rendu
  par `GET /ontology/changes/head` ;
- permission `ontology:read` ; MCP exposé (lecture JSON).

**Purge** : tâche quotidienne, lignes plus vieilles que la rétention.

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
   de sa détection, et non vers les captures suivantes de la page (Q-A3).

Dans le pack `quali_watch` (seule partie propre au métier) :
`constate_dans` et `prouve_par` passent `evidence: true`. Les règles
n'ont rien d'autre à faire.

## 8. Client généré (D438)

### D438 — Un client Rust typé, tiré des types de l'org

`pnex-mirror` (D439) contient un générateur, `pnex-app-gen`, qui lit les
types de l'org (`GET /ontology/types`, `ontology.schema:read`) dans le
périmètre de l'application et écrit un module Rust :

- une struct par type d'objet (propriétés typées : texte, nombre, date,
  enum, ref, booléen ; `Option` si non requise), avec `id`, `title`,
  `version`, `provenance` ;
- pour chaque type de lien du périmètre, un accès typé
  (`article.ecrit_par(&mirror)` → `Vec<Auteur>`) qui lit le miroir ;
- une fonction par ensemble d'objets nommé ;
- la `type_version` de chaque type, vérifiée à l'exécution : un écart
  donne un avertissement, jamais une panique (un pack monté ajoute des
  propriétés).

Le module généré est commité dans l'application et régénéré à la montée
de pack (`pnex-app-gen --org <id> --app <key> --out src/ontology.rs`).
TypeScript au lot 2, à partir du même modèle intermédiaire.

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
- un **exemple** minimal (`examples/articles.rs`) et des tests sur une
  ontologie inventée (`article`, `auteur`, `ecrit_par`) avec un faux
  serveur local.

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
  `services/ontology/packs.rs`).

**Constat du 2026-10-25** : aucune violation par nom dans le code non
test. Restent, à traiter à part :

- le nœud `org_resolve` (nom → SIREN) et le mode d'import `datagouv` :
  des connecteurs à des sources publiques françaises, génériques pour
  tout pack mais propres à un pays ; à ranger comme connecteurs
  (fournisseur déclaré, comme la géo) plutôt que dans le moteur ;
- des modules de test dans `src` (`ontology/flow.rs`,
  `ontology/import.rs`, `ingest.rs`, `controllers/flows/artifact.rs`,
  `pnex-notify`) et un test du front qui inclut `quali_watch.yaml` :
  admis (tests), mais à migrer vers une ontologie inventée quand on y
  touche.

## 12. Lots

| Lot | Contenu | Tests |
|---|---|---|
| **A0** | D432 principe + D441 garde | `engine_genericity.rs` |
| **A1** | D435 : table, triggers, curseur, `/ontology/changes` (+ `head`), purge | ordre et absence de trou avec deux transactions croisées ; import en masse ; objet natif ; 410 |
| **A2** | D433 : `apps` (CRUD, `apps/me`, périmètre appliqué au flux) | 403 sans permission ; périmètre filtrant |
| **A3** | D434 : ensembles d'objets nommés, paramètres | substitution des valeurs seulement ; périmètre |
| **A4** | D437 : `evidence: true`, `/objects/{id}/evidence`, lien direct au scellement ; pack `quali_watch` | écart → page → pièces sur une ontologie inventée |
| **A5** | D439 : `pnex-mirror` (client, flux, SQLite, médias, projection, exemple) | faux serveur ; coupure et reprise ; sha256 faux refusé ; 304 |
| **A6** | D438 : `pnex-app-gen` Rust | module généré compilé dans les tests |
| **A7** | D421 : documents d'une page dans `site_crawl` (config `record.documents`), générique | serveur de test local |
| Lot 2 | D440 actions, SSE, agrégats, TypeScript | — |

Vigie bascule après A5 (A6 facultatif pour elle).

## 13. Questions pour Shan (défaut retenu en attendant)

- **Q-A1 — Périmètre** : appliqué par le serveur au flux et aux
  ensembles d'objets seulement (défaut), ou aussi à toutes les routes
  de lecture quand l'appelant est une application ?
- **Q-A2 — Rétention du flux** : 30 jours (défaut), puis copie complète.
- **Q-A3 — Preuves** : liens `evidence` et lien direct au scellement
  (défaut), ou faire de la preuve web un objet natif de l'ontologie
  (`web_proof`, table système comme les médias) ?
- **Q-A4 — Écritures des applications** : assouplir la règle « lecture
  seule » des comptes de service pour les seuls comptes d'application et
  les seules actions déclarées (défaut, lot 2) ?
- **Q-A5 — Types du SDK** : types propres à `pnex-mirror`, vérifiés
  contre le spec OpenAPI (défaut), ou dépendance à `pnex-core` (plus
  lourde pour une application) ?
- **Q-A6 — Client généré** : par application et par org (défaut), ou par
  pack (le même client pour toutes les orgs qui ont le pack) ?
- **Q-A7 — Connecteurs français** (`org_resolve`, `datagouv`) : les
  ranger en connecteurs déclarés (défaut, chantier séparé) ou les laisser
  au moteur ?
