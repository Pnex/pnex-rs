# Runtime : construction, images, migrations, montée en charge, administration

## 1. Résumé

Ce document décrit comment PneX est construit, emballé, migré, mis à l'échelle et administré. Il s'adresse aux développeurs et aux exploitants d'une instance. Le cœur est un workspace Rust : un serveur Loco, un front Dioxus CSR et des crates partagées. Le serveur tourne dans des images Chainguard, sur une base PostgreSQL unique, en un ou plusieurs pods. Un admin plateforme règle la rétention des données et lit le statut de l'instance.

## 2. Concepts

- **Crate partagée** : crate compilée en natif et en wasm32 (`pnex-core`, `pnex-api-contract`). Elle dépend de serde seulement.
- **Serveur** : binaire `pnex-server` de la crate `pnex-backend` (API HTTP, WebSocket, serving du front, file de jobs).
- **Front** : crate `pnex-frontend`, UI Dioxus en rendu client seul (CSR). Cibles : web (wasm32), Android, desktop.
- **Contrat** : entier `CONTRACT` de `pnex-api-contract`. Un bump signale une rupture d'API.
- **Image** : image conteneur livrée (`pnex-server`, `pnex-builder`, `pnex-pipeline-worker`, `pnex-runner`).
- **Migration de base** : migration `m20261009_000001_baseline`, qui crée tout le schéma initial (D120).
- **Pod API** : processus serveur qui sert HTTP et WebSocket.
- **Pod flow-worker** : processus serveur qui exécute les flows des orgs placées chez lui (D106).
- **Worker de file** : processus `pnex-server start --worker`, qui exécute les jobs en base (builds firmware, stitch).
- **Bail** : droit exclusif et temporaire d'un processus sur une ressource (présence device, tâche singleton).
- **Admin plateforme** : utilisateur avec `users.platform_admin`. Il administre toute l'instance, hors des rôles d'org.
- **Rétention effective** : durée de conservation des métriques d'une org dans OpenObserve (O2).

```mermaid
---
config:
  layout: elk
---
flowchart LR
  core[pnex-core] --> backend[pnex-backend]
  contract[pnex-api-contract] --> backend
  core --> front[pnex-frontend]
  contract --> front
  backend --> server[image pnex-server]
  backend --> builder[image pnex-builder]
  front --> server
  pw[pnex-pipeline-worker] --> pwimg[image pnex-pipeline-worker]
  runner[pnex-runner] --> runimg[image pnex-runner]
  server --> pg[(PostgreSQL)]
  builder --> pg
  server --> valkey[(Valkey)]
  server --> s3[(stockage S3)]
```

## 3. Fonctionnement

### 3.1 Workspace et features

Le workspace compte plus de 35 crates (`Cargo.toml`, `members`). Les crates principales sont :

| Crate | Rôle | Cibles |
|---|---|---|
| `pnex-core` | DTO et constantes partagés. Aucune dépendance native (ni tokio, ni `std::net`, ni `std::fs`). | natif et wasm32 |
| `pnex-api-contract` | Contrat app ↔ serveur : `CONTRACT`, `SERVICE`, `ServerInfo`, `compatible(a, b)` (égalité stricte). | natif et wasm32 |
| `pnex-backend` | API Loco/Axum, WebSocket, workers, serving du front. Binaire `pnex-server`. | natif |
| `pnex-frontend` | UI Dioxus CSR. Elle appelle l'API en HTTP et WebSocket. | wasm32, Android, desktop |
| `pnex-firmware-builder` | Orchestration des builds firmware. | natif |
| `pnex-node-*` | Nœuds de flow PneX (`flow-engine.md` §3.7). | natif |
| `pnex-flow-runtime` | Runtime de flows, supervisé par le serveur. | natif |
| `pnex-pipeline-worker` | Transformations SQL des pipelines (DataFusion). | natif |

Features du front (`crates/pnex-frontend/Cargo.toml`) :

| Feature | Effet |
|---|---|
| `web` (défaut) | `dioxus/web` |
| `mobile` | `dioxus/mobile`, cible Android |
| `desktop` | `dioxus/desktop` (webkit2gtk sous Linux, WebView2 sous Windows) |
| `router` | `dioxus/router` |
| `diag` | diagnostics sur l'appareil, jamais dans une build livrée |
| `e2e` | crochets du harnais e2e natif, builds de test seulement |

Le front n'active jamais les features Dioxus `server` ou `fullstack`. Le backend est Loco, lui aussi sur Axum : deux piles Axum pour une application se composent mal. Le rendu serveur exige aussi un rendu identique entre serveur et wasm, ce qui est fragile. Le front parle donc au backend en HTTP et WebSocket, comme un client externe, un device ou un test.

Le style vient de Tailwind CSS v4 (`style/tailwind.css` → `assets/tailwind.css`, par `task css:build`). L'i18n vient de Fluent (`dioxus-i18n`, `locales/{fr-FR,en-US}.ftl`). Le test `i18n::tests::fr_en_key_parity` vérifie la parité des clés.

### 3.2 Serving du front

```mermaid
---
config:
  layout: elk
---
flowchart LR
  dx[dx build --platform web] --> out[target/dx/.../public]
  out -->|Taskfile| dist[crates/pnex-frontend/dist]
  dist --> loco[middleware static de Loco]
  loco -->|fallback index.html| nav[navigateur]
```

En production, Loco sert `dist` avec un repli sur `index.html` (SPA). En développement, `task dev:hot` lance `dx serve` sur :5151 et `task css:watch`. Le backend tourne à part (`task dev:backend`, :5150). Le CORS vers l'origine :5151 existe dans `development.yaml` seulement.

Les tâches `build:frontend`, `build:frontend:android` et `dev:frontend` incluent `pnex-api-contract` dans leurs `sources`. Un bump du contrat reconstruit donc le front.

### 3.3 Clients natifs : URL du serveur et porte de compatibilité

Sur Android et desktop, le serveur ne sert pas le front. L'utilisateur saisit l'URL de son serveur auto-hébergé.

```mermaid
---
config:
  layout: elk
---
flowchart TD
  boot[démarrage de l'app] --> base{api_base vide ?}
  base -->|oui| url[écran ServerUrl]
  url -->|saisie ou scan LAN| apply[apply_server]
  base -->|non| gate[porte GATE]
  apply --> gate
  gate -->|GET /api/v1/meta/version| cmp{contrat égal ?}
  cmp -->|oui| router[routeur]
  cmp -->|non| block[écran bloquant]
  gate -->|injoignable ou non PneX| retry[écran bloquant + réessai]
```

- `api/config.rs` résout l'URL de base. Web : URL relative (même origine). Natif : variable d'environnement, puis préférence stockée `pnex.api_base`.
- `apply_server` (`pages/server_url.rs`) est le chemin unique : sonde, persistance, porte, `SERVER_READY`. Les trois points d'entrée l'utilisent (`ServerUrl`, `SelfHostedSection` du login, scan LAN).
- Le scan LAN (`pages/lan_scan.rs`, natif seulement) sonde `http://{ip}:5150` sur un /24. Il garde les réponses `pnex-server`. La plage vient de l'URL courante, sinon de `java.net.NetworkInterface`, sinon de `192.168.1.` éditable.
- `storage::local()` persiste l'URL, les tokens et l'org courante dans `pnex-storage.json`, dans le dossier privé de l'app. Les écritures sont atomiques. Un fichier corrompu donne un démarrage à vide. Le verifier PKCE reste en mémoire.
- `version.rs` affiche la version et le `BUILD_SHA` au pied du login et dans la carte « À propos » du profil.

Le login natif passe par le navigateur système. dioxus-desktop ouvre toute navigation http(s) dans ce navigateur : une webview embarquée est donc impossible sans fork.

```mermaid
sequenceDiagram
  participant App
  participant Nav as Navigateur système
  participant Back as Backend
  participant IdP as Rauthy
  App->>App: PKCE + state
  App->>Nav: ouvrir /api/v1/oauth2/sso
  Nav->>IdP: login
  IdP->>Back: redirect /api/v1/oauth2/native?code&state
  Back->>Back: mémoriser state → code (5 min)
  loop sondage
    App->>Back: GET /native/{state}
  end
  Back-->>App: code
  App->>IdP: échange du code (verifier local)
```

Le verifier PKCE ne quitte jamais l'app. Le `state` est le secret de capacité du pont. Chaque URL `{base}/api/v1/oauth2/native` doit figurer dans les `redirect_uris` du client `pnex` de Rauthy.

Pièges connus du développement Android :

| Symptôme | Cause | Geste |
|---|---|---|
| SIGABRT au lancement ou à l'ouverture du navigateur | double init `ndk-context`, macros jni de `webbrowser` | crates vendues et patchées (`vendor/patches/README.md`) |
| « Invalid credentials » en boucle | Chrome Android refuse les cookies `__Host-` en http | `COOKIE_MODE=danger-insecure` dans `compose.yaml` (dev seulement) |
| Login bloqué plusieurs minutes | le rate-limit de Rauthy ne voit que l'IP de la passerelle Docker | attendre l'expiration, ne rien relancer |
| Le pont expire | le freezer Android gèle l'app en arrière-plan | revenir vite dans l'app |
| L'app meurt après `am start` | l'Activity est recréée dans le même process | `force-stop` avant `am start` |

**Attention :** le client HTTP vit en `thread_local`, car les futurs reqwest sont `!Send` en wasm. Si un runtime natif exige des futurs `Send`, la couche HTTP doit changer.

### 3.4 Images conteneur (D71)

```mermaid
---
config:
  layout: elk
---
flowchart LR
  assets[assets : bun, Tailwind, esbuild] --> web[web : dx build]
  tc[toolchain : rust trixie, dx] --> web
  tc --> build[build : pnex-server, runtime]
  tc --> pwb[pipeline-worker-build]
  tc --> rb[runner-build]
  web --> server[server : glibc-dynamic]
  build --> server
  build --> builder[builder : wolfi-base]
  pwb --> pw[pipeline-worker : glibc-dynamic]
  rb --> runner[runner : glibc-dynamic]
```

`deploy/docker/Dockerfile` construit les images depuis la racine du repo.

| Image | Base | Contenu | Commande |
|---|---|---|---|
| `pnex-server` | `cgr.dev/chainguard/glibc-dynamic` : ni shell ni gestionnaire de paquets, utilisateur 65532 | `pnex-server`, `pnex-flow-runtime`, front `dist`, ffmpeg, `pnex-asr`, fixtures | `start` |
| `pnex-builder` | `cgr.dev/chainguard/wolfi-base` (glibc) | même binaire, PlatformIO 6.2.0, esptool 5.4.0, toolchains ESP8266/ESP32/C3/S3/C6 préchauffées, modèles ONNX | `start --worker` |
| `pnex-pipeline-worker` | `glibc-dynamic` | binaire seul, sans configuration ni secret applicatif | — |
| `pnex-runner` | `glibc-dynamic` | runner de la fabric de workers | — |

Le worker de build lance `pio` et `esptool` (Python, toolchains glibc) : il ne peut pas tourner sans shell. Le serveur dépend seulement de glibc et libstdc++. Les étapes de build utilisent Debian trixie, car le `dx` précompilé exige glibc ≥ 2.39. Les images Chainguard livrent glibc 2.44 (compatible vers le haut). Alpine est écarté (musl, alors que les toolchains PlatformIO sont glibc).

Les images de l'environnement d'agents (`image-builder`, `image-scanner`, `sandbox-init`) sont décrites dans `agent-environments.md`.

Exécution locale : `compose.app.yaml` ajoute `pnex-server` et `pnex-builder` à l'infra de `compose.yaml`. `LOCO_ENV=production` charge `production.yaml` (migration au boot, issuer, flows actifs, outils de debug coupés). nginx de l'edge résout ses upstreams à chaque requête : il suit la nouvelle IP d'un conteneur recréé.

### 3.5 Migrations de schéma

La migration de base `m20261009_000001_baseline` exécute `migration/src/baseline/postgres.sql`. Le fichier `tables.txt` donne l'ordre de création, utilisé par `down()`. Chaque changement de schéma depuis la base est une nouvelle migration (`m20261011_000002_ontology` et suivantes).

Règles pour chaque nouvelle migration :

1. Un fichier par changement, nommé `mAAAAMMJJ_NNNNNN_sujet.rs`, avec un numéro strictement croissant. Une migration publiée ne change jamais : une nouvelle migration la corrige.
2. Additive d'abord (expand/contract). Pour retirer ou renommer, le code cesse d'abord de lire l'élément, puis une migration ultérieure le supprime.
3. SQL PostgreSQL natif, sans branche par moteur.
4. Énumérations : ajouter une valeur à un `ENUM` existant par `ALTER TYPE … ADD VALUE IF NOT EXISTS`. Une nouvelle énumération est de préférence un `varchar` validé par l'application.
5. Conventions : tables au pluriel, `org_id bigint NOT NULL` et FK `ON DELETE CASCADE` pour une donnée d'org (D2), `created_at` et `updated_at timestamptz DEFAULT CURRENT_TIMESTAMP NOT NULL`, index `idx_<table>_<colonnes>`, uniques `uniq_<table>_<colonnes>`, FK `fk-<table>-<colonne>`.
6. Pas de donnée métier dans une migration : le catalogue est semé au boot (`PNEX_AUTO_SEED`). Un backfill va dans la migration qui en a besoin, idempotent.
7. `down()` existe pour une migration additive. Une migration destructive dit qu'elle n'est pas réversible.
8. Après la migration, régénérer les entités (`task db:entities`) et lancer les tests de `pnex-migration` (`schema_invariants`).

PostgreSQL est le moteur unique (décision #19 de `roadmap.md`). `DATABASE_URL` doit pointer sur PostgreSQL. Une URL `sqlite://` n'est pas prise en charge. La file de jobs Loco vit dans la même base (`queue.kind: Postgres`). `sqlx-sqlite` reste compilé par transitivité (feature `with-db` de `loco-rs`) : ne pas l'utiliser.

Le code peut utiliser tout le SQL PostgreSQL : JSONB (`@>`, `?`), index GIN et GiST, `WITH RECURSIVE`, `ON CONFLICT`, `ILIKE`, `now()`. Une FK, même circulaire, s'ajoute par `ALTER TABLE`. Une extension s'active par migration (`CREATE EXTENSION IF NOT EXISTS`), et sa licence passe la règle des briques permissives. Une requête brute reste paramétrée (`$1`…).

Tests : tous les tests de base passent par `TEST_DATABASE_URL` (défaut `postgres://pnex:pnex@localhost:5432/pnex_test`). Chaque boot de test vide la base, et les tests qui bootent l'app restent `#[serial]`. Un test unitaire qui a besoin d'un schéma appelle `db_lock::migrated_test_db()`. Un worktree ou une session parallèle utilise sa propre base (`CREATE DATABASE pnex_test_<nom> OWNER pnex`).

### 3.6 Montée en charge horizontale

La cible est N pods `pnex-server` derrière un load balancer sans affinité de session, une base PostgreSQL, Valkey et un stockage S3. Le cluster de flows (D106) et le bus device (D107) sont dans `flow-engine.md` §3.4 et §3.5.

```mermaid
---
config:
  layout: elk
---
flowchart LR
  lb[load balancer] --> api[pods API]
  api -.->|/internal/flow-cluster| fw[pods flow-worker]
  api --> pg[(PostgreSQL)]
  fw --> pg
  qw[workers de file : pnex-builder] --> pg
  api --> valkey[(Valkey)]
  fw --> valkey
  api --> s3[(S3)]
  qw --> s3
```

| Rôle | Réglage | Remarque |
|---|---|---|
| Pod API | `PNEX_FLOW_RUN_WORKER=false` | Un pic de flows ne dégrade pas l'API. |
| Pod flow-worker | `PNEX_FLOW_RUN_WORKER=true` (défaut), `PNEX_SERVES_API=false` | Le chart `pnex-deploy` le crée quand `flowWorker.replicas > 0`. Sinon, les pods API exécutent aussi les flows. |
| Worker de file | `pnex-server start --worker-only` ou `--server-and-worker` | Image `pnex-builder`, là où vit PlatformIO. |

Tous les pods partagent :

- le même `PNEX_FLOW_CLUSTER_TOKEN` (routes `/internal/flow-cluster/*`, refus par défaut) ;
- `PNEX_FLOW_ADVERTISE_URL=http://$(POD_IP):5150` sur chaque pod worker ;
- `VALKEY_URL`, obligatoire même en mono-pod (D108), avec l'éviction `volatile-lru` ;
- le stockage S3 (`PNEX_S3_*`, RustFS de référence). Le serveur n'a pas de stockage disque local.

**Migration au boot.** `PNEX_AUTO_MIGRATE` (vrai en production) fait migrer chaque pod. `App::boot` appelle `db_lock::migrate_under_lock` avant `create_app` de Loco.

```mermaid
sequenceDiagram
  participant P1 as Pod 1
  participant P2 as Pod 2
  participant DB as PostgreSQL
  P1->>DB: pg_advisory_lock(PNEXMIGR)
  P2->>DB: pg_advisory_lock(PNEXMIGR)
  Note over P2: attend
  P1->>DB: migrations
  P1->>DB: unlock
  DB-->>P2: verrou
  P2->>DB: rien à appliquer
```

Un Job k8s `pnex-server db migrate` avec `PNEX_AUTO_MIGRATE=false` reste possible. Une migration reste compatible avec la version N-1 encore en service pendant le rollout.

**Arrêt propre.** `App::on_shutdown` enchaîne trois étapes.

```mermaid
sequenceDiagram
  participant K as Kubernetes
  participant P as Pod
  K->>P: SIGTERM (après preStop 10 s)
  P->>P: drain du worker de flows (≤ 20 s)
  P->>P: vidage des écrivains bufferisés (≤ 10 s)
  P->>P: libération des baux task:*
  K->>P: SIGKILL si grace dépassée
```

Le drain déplace les orgs vers un autre worker (`drain_timeout_ms`). Le vidage (`services/drain.rs`) envoie le dernier lot de télémétrie vers O2 et le journal notify. La libération (`singleton::release_all`) laisse un autre pod reprendre les sweeps sans attendre l'expiration. Le total atteint 30 s au pire. `terminationGracePeriodSeconds` doit donc valoir 40 s ou plus. Le chart ajoute un `preStop.sleep` de 10 s (grace 45 s), pour que l'ingress cesse de router vers le pod.

**Pools de connexions.**

| Clé | Défaut | Variable |
|---|---|---|
| `database.connect_timeout`, `acquire_timeout` | 5 000 ms | `DB_CONNECT_TIMEOUT`, `DB_ACQUIRE_TIMEOUT` |
| `database.idle_timeout` | 300 000 ms | `DB_IDLE_TIMEOUT` |
| `database.max_connections` | 10 | `DB_MAX_CONNECTIONS` |
| `queue.max_connections` | 2 | `PNEX_QUEUE_MAX_CONNECTIONS` |

`pods × (database.max_connections + queue.max_connections)` doit rester sous le `max_connections` de PostgreSQL, moins la marge admin. Chaque deploy en cours tient une connexion de plus (verrou par org). Au-delà de quelques dizaines de pods, utiliser pgbouncer en mode *session*. Le mode *transaction* casse les verrous consultatifs.

**Cohérence sous concurrence.** La base est la source de vérité. Aucune garde « lire puis écrire » ne dépend de la mémoire d'un pod.

| Chemin | Mécanisme |
|---|---|
| Deploy, rollback, start, stop, delete de flow | verrou consultatif par org (`FLOW_DEPLOY`). Attente bornée à 90 s, puis 503 `flow-runtime`. |
| Projection des régulations | recalcul par org dans une transaction, sous verrou par org |
| OTA | index unique partiel « une assignation active par device » → 409 `ota-in-progress`. Transitions compare-and-set (`WHERE state = <attendu>`). |
| Sauvegardes versionnées (flows, tours, calques d'annotations, dashboards) | contrôle optimiste dans la transaction. Une violation d'unicité donne 409, jamais 500. |
| Quotas (devices, builds) | `COUNT` et insertion sous verrou par org |
| Baux `flow_leases` | expiration calculée avec l'horloge de la base (`now()`) |
| Stitch 360 | claim conditionnel. Timeout `PNEX_STITCH_TIMEOUT_SECS` (1 500 s). |
| Purge O2 après suppression d'org | réessais de 5 s à 10 min, puis erreur journalisée |

### 3.7 Présence device en Valkey (D108)

```mermaid
stateDiagram-v2
  [*] --> Libre
  Libre --> Tenu: claim (Lua, first-live-wins)
  Tenu --> Tenu: touch (propriétaire vérifié)
  Tenu --> Libre: release ou expiration du TTL
  Tenu --> Inactif: reaper (silence)
  Inactif --> Tenu: claim
```

Le bail est la clé `pnex:{db}:live:lease:{device}`, avec une expiration égale au TTL de silence. Le ZSET `pnex:{db}:live:seen` garde `last_seen`. PostgreSQL reçoit seulement les transitions : `active` et `device_states.last_seen_at`. Le reaper est le seul écrivain de `active`. Une lecture d'API fusionne Valkey et PostgreSQL, et la valeur la plus fraîche gagne.

Après un redémarrage de Valkey, le marqueur `epoch` est perdu. Le reaper suspend alors les désactivations pendant un TTL de silence. Pendant cette fenêtre, un clone peut prendre un bail libéré : la limite est acceptée.

### 3.8 Administration système (D72)

| Rôle | Portée | Droits |
|---|---|---|
| Admin plateforme | instance | statut plateforme, défaut de rétention, override par org, rechiffrement des secrets |
| Owner ou admin d'org | son org | nettoyage O2 (stream, plage, purge) |
| Viewer | son org | lecture de la rétention effective et des streams |

`PNEX_PLATFORM_ADMIN_EMAILS` promeut au login les utilisateurs listés. Il ne les rétrograde jamais : la base fait foi. Un admin plateforme n'est pas membre implicite des orgs.

```mermaid
---
config:
  layout: elk
---
flowchart TD
  mode{PNEX_DEPLOYMENT_MODE}
  mode -->|self_hosted| a1[override d'org]
  a1 --> a2[défaut plateforme system_settings]
  a2 --> a3[PNEX_DEFAULT_RETENTION_DAYS = 30]
  mode -->|saas| b1[override d'org]
  b1 --> b2[tier d'abonnement data_retention_secs]
  b2 --> b3[variable d'environnement]
  a3 --> o2[stream O2 : jours, minimum 1]
  b3 --> o2
```

La première valeur présente gagne. O2 compte la rétention par stream, en jours entiers, avec 1 jour au minimum. L'UI signale une valeur relevée. Le serveur réconcilie tous les streams des orgs provisionnées chaque heure, et tout de suite après un changement de réglage. Une valeur `0` côté O2 veut dire « défaut de l'instance O2 ».

Nettoyage O2 :

| Action | Route PneX | Appel O2 |
|---|---|---|
| Supprimer un stream | `DELETE /api/v1/system/o2/streams/{name}` | `DELETE /api/{org}/streams/{name}?type=metrics` |
| Supprimer une plage | `POST …/streams/{name}/delete-range` | `DELETE …/data_by_time_range` |
| Purger l'org | `POST /api/v1/system/o2/purge` (nom de l'org retapé) | suppression de chaque stream |

O2 exige un début de plage aligné sur l'heure. Le serveur arrondit donc le début vers le bas et la fin vers le haut. Le compacteur traite la suppression plus tard : la réponse veut dire « planifiée ». Les appels de gestion utilisent le compte root d'O2. À la suppression d'une org PneX, le serveur lit l'identifiant O2 avant la cascade, puis purge les streams en tâche de fond.

**Statut plateforme.** `GET /api/v1/system/status` lance des sondes en parallèle, chacune bornée à 3 s. Chaque composant a un statut `ok`, `degraded`, `down` ou `not_configured`.

| Composant | Sonde | Mesures |
|---|---|---|
| Base | `SELECT 1` | taille (`pg_database_size`), 5 plus grosses tables |
| Rauthy | `/auth/v1/health`, `/auth/v1/version` | version, issuer |
| OpenObserve | `/healthz`, streams des orgs | stockage, points, streams |
| Valkey | `PING`, `INFO` | mémoire, version |
| Stockage objets | `Operator::check()` par bucket | tailles par le catalogue en base, jamais de listing de bucket |
| Topologie | environnement, abonnés `pnex:devcmd:v1:*`, `flow_workers` vivants, bail `flow-placement` | un nœud par ligne avec son rôle. Dégradé sans flow-worker vivant ou sans contrôleur. |
| Machine hôte | `statvfs`, `/proc`, device-tree, DMI | disque, RAM, charge, type (Raspberry Pi, VM, conteneur) |
| Service IA | fournisseurs d'org (D119) | nombre de fournisseurs |
| Coffre de secrets | trousseau `PNEX_SECRETS_KEYS`, `org_secrets` | clés, secrets sous une ancienne clé (`security.md`) |

Les workers de file ne publient pas de présence : la topologie ne les liste pas.

`GET /api/v1/system/orgs` liste chaque org avec son abonnement, sa rétention effective et sa source, son stockage O2 et son quota. La page `/admin/status` l'affiche, avec l'override éditable en ligne.

**Quotas de stockage.** `subscription_tiers.max_telemetry_mb` (null = illimité) s'applique en SaaS seulement. L'usage est la somme des `compressed_size` O2 de l'org. Le quota est informatif : une jauge sur `/system` et une alerte au dépassement. La collecte n'est jamais bloquée.

### 3.9 Version de PostgreSQL

L'image de référence est `postgis/postgis:18-3.6-alpine` (`compose.yaml`, `.github/workflows/ci.yml`). En cluster, l'image CNPG PostGIS est utilisée. Le graphe reste « jointures + CTE + `petgraph` » (`ontology.md` annexe A3).

PostgreSQL 19 n'apporte pas de graphe natif : SQL/PGQ a été retiré avant la GA, au plus tôt en PG 20. `UPDATE … FOR PORTION OF` est absent. Les apports utiles sont :

| Nouveauté PG 19 | Usage dans PneX |
|---|---|
| `range_minus_multi()` | trous entre deux périodes d'un lien, vue Histoire des dossiers |
| `INSERT … ON CONFLICT DO SELECT … RETURNING` | get-or-create d'objet `(org_id, type_key, native_id)` et imports, sans `DO UPDATE` factice |
| fenêtres `IGNORE NULLS` | dernière valeur connue d'une propriété temporelle |
| `REPACK (CONCURRENTLY)` | réorganiser `resource_edges` et `objects` en ligne (D179 : l'historique ne supprime jamais) |

```mermaid
---
config:
  layout: elk
---
flowchart LR
  m0[M0 : job CI 19 non bloquant] --> m2[M2 : compose + CI en 19]
  m1[M1 : vérifs de rupture] --> m2
  ga{GA de PG 19} --> m2
  m2 --> m3[M3 : pg_upgrade du chart et de dev]
  m3 --> m4[M4 : adoption des nouveautés]
  m4 --> m5[M5 : veille SQL/PGQ en PG 20]
```

M1 vérifie : pas d'index `btree_gist` sur `inet`/`cidr`, pas d'auth MD5, pas de CR/LF dans les noms de base ou de rôle. Aucune fonctionnalité de M4 ne doit casser PG 18 avant la fin de M3 partout. Si PostGIS ou pgvector sont montés avant M2, M2 attend leurs images 19 arm64.

## 4. Exigences

1. `pnex-core` et `pnex-api-contract` doivent compiler en natif et en wasm32 (`task check`, CI).
2. Le backend ne doit jamais dépendre du front. Le front doit dépendre seulement des crates partagées.
3. `pnex-frontend` ne doit activer aucune feature Dioxus serveur.
4. Un bump de `CONTRACT` doit bloquer un client d'un autre contrat avec un écran explicite.
5. L'image `pnex-server` ne doit contenir ni shell ni gestionnaire de paquets, et doit tourner en non-root.
6. Une migration publiée ne doit jamais changer. Une nouvelle migration doit rester compatible avec la version N-1.
7. Toute donnée d'org doit porter `org_id NOT NULL` avec une FK `ON DELETE CASCADE`.
8. Une constante de verrou de `db_lock::ns` ne doit jamais être réattribuée, même retirée.
9. Un seul pod doit appliquer les migrations à la fois (verrou `PNEXMIGR`).
10. `terminationGracePeriodSeconds` doit valoir 40 s ou plus.
11. Tout déploiement doit fournir `VALKEY_URL`, sinon le serveur refuse de démarrer.
12. `pods × (database.max_connections + queue.max_connections)` doit rester sous `max_connections` de PostgreSQL.
13. Une sauvegarde versionnée concurrente doit répondre 409, jamais 500.
14. La sonde de stockage ne doit jamais lister un bucket.
15. Le dépassement de quota ne doit jamais bloquer la collecte.

## 5. Décisions

- **D71** : images Chainguard. `pnex-server` sur `glibc-dynamic`, `pnex-builder` sur `wolfi-base`. Alpine et CentOS écartés.
- **D72** : admin plateforme, rétention O2 par mode de déploiement, nettoyage O2, statut plateforme.
- **D108** : bail de présence device en Valkey. Valkey devient obligatoire. Révise D9.
- **D120** : une migration de base remplace les migrations de développement. Une base antérieure doit être recréée (`task db:reset`).
- **Décision #19** (`roadmap.md`) : PostgreSQL seul, SQLite retiré.
- Renvois : D2 (org), D106 et D107 (`flow-engine.md`), D119 (pas de LLM plateforme), D179 (historique des liens), D220 (lecteur temporel).

## 6. Interfaces

Routes :

| Route | Rôle |
|---|---|
| `GET /api/v1/meta/version` | public : `ServerInfo { service, version, contract }` |
| `GET /api/v1/oauth2/sso`, `/native`, `/native/{state}` | pont de login natif |
| `GET /api/v1/system/status` | admin plateforme |
| `GET /api/v1/system/orgs`, `PUT /api/v1/system/retention/orgs/{org_id}` | admin plateforme |
| `GET /api/v1/system/retention`, `PUT …/retention/default` | membre (lecture), admin plateforme (écriture) |
| `GET /api/v1/system/o2/streams`, `DELETE …/{name}`, `POST …/{name}/delete-range`, `POST /api/v1/system/o2/purge` | owner ou admin d'org |
| `POST /api/v1/system/secrets/rekey` | admin plateforme |

Tâches :

| Tâche | Effet |
|---|---|
| `task build:frontend`, `build:frontend:android`, `build:frontend:android:release`, `build:frontend:linux` | builds du front |
| `task docker:build [TAG=…]`, `task docker:publish` | images locales, ou push multi-arch amd64 + arm64 |
| `task app:up [EDGE=1]`, `task app:down` | PneX en conteneurs, avec ou sans edge TLS |
| `task db:reset`, `task db:entities` | recréer la base, régénérer les entités |

Variables : `DATABASE_URL`, `TEST_DATABASE_URL`, `VALKEY_URL`, `PNEX_S3_*`, `PNEX_AUTO_MIGRATE`, `PNEX_AUTO_SEED`, `PNEX_FLOW_RUN_WORKER`, `PNEX_SERVES_API`, `PNEX_FLOW_CLUSTER_TOKEN`, `PNEX_FLOW_ADVERTISE_URL`, `DB_*`, `PNEX_QUEUE_MAX_CONNECTIONS`, `PNEX_STITCH_TIMEOUT_SECS`, `PNEX_PLATFORM_ADMIN_EMAILS`, `PNEX_DEPLOYMENT_MODE`, `PNEX_DEFAULT_RETENTION_DAYS`, `PNEX_SECRETS_KEYS`.

## 7. Reste à faire

- iOS : décision de phase explicite à prendre (Shan).
- Tags Chainguard gratuits = `latest` : épingler `server` et `builder` par digest pour des builds reproductibles.
- Les `lib_deps` PlatformIO se téléchargent à chaque build : le builder a besoin du réseau.
- Heartbeats des flow-workers et `is_alive` comparés avec l'horloge des pods (dérive tolérée = TTL 10 s).
- Purge O2 après suppression d'org non durable à travers un redémarrage (table de purges en attente + sweep).
- Le stitch bloquant (`spawn_blocking`) n'est pas interruptible : au timeout, le thread finit en arrière-plan.
- Dette de portabilité : `lower(col) LIKE` → `ILIKE` (`global_search.rs`, `pagination.rs`), `GROUP BY` dans `dashboard.rs::build_stats`.
- Blocage ou dégradation de l'ingestion au dépassement de quota : décision produit (Shan).
- Test e2e matériel sur Raspberry Pi (détection `raspberry_pi`, disque SD).
- Histoire du graphe, sur PG 18 : graphe à une date (`as_of` dans `services/ontology/graph.rs`), défilement des instants de changement, diff entre deux dates.
- Liens clos qui se chevauchent : les interdire (`WITHOUT OVERLAPS`, `btree_gist`) ou les signaler à l'analyste. Choix de Shan.
- Passage à PG 19 : étapes M0 à M5 (§3.9), M2 après la GA.
