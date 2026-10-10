# Scalabilité horizontale — exigences opérationnelles et cohérence base

> **Statut : 2026-10-01, chantier « scale » (audit multi-pods, workstream D).**
> Complète `flow-engine.md` §7 (cluster d'exécution D106, bus device D107).
> Cible : N pods `pnex-server` derrière un load balancer **sans affinité de
> session**, une seule base Postgres, Valkey, un stockage S3-compatible.

## 1. Topologie recommandée

| Rôle | Réglage | Remarques |
|---|---|---|
| **Pods API** | `PNEX_FLOW_RUN_WORKER=false` | Servent HTTP/WS (UI, devices, ingestion). Pas de runtime de flows : un pic de flows ne dégrade pas l'API. |
| **Pods flow-worker** | `PNEX_FLOW_RUN_WORKER=true` (défaut) | Exécutent les flows des orgs placées chez eux (D106). Peuvent aussi servir HTTP, mais isoler les rôles simplifie le dimensionnement. |
| **Workers de queue** (builds firmware, stitch 360, checks firmware) | `pnex-server start --worker-only` (ou `--server-and-worker`) | Doivent tourner là où vit PlatformIO (image `pnex-builder`). |

Tous les pods partagent :

- `PNEX_FLOW_CLUSTER_TOKEN` : **le même** secret partout (routes internes `/internal/flow-cluster/*`, fail-closed).
- `PNEX_FLOW_ADVERTISE_URL=http://$(POD_IP):5150` sur chaque pod worker (adresse joignable par les autres pods ; `POD_IP` via l'API downward k8s).
- `VALKEY_URL` / `settings.valkey.url` : **obligatoire dans tous les déploiements, mono-pod compris** (D108 : baux de présence device ; sans URL le serveur refuse de démarrer). Sert aussi au bus de commandes device D107, au cache live, à la mémoire d'org et au bus caméra. Politique d'éviction : `volatile-lru` (jamais `allkeys-lru`, qui peut évincer le ZSET de présence).
- **Stockage S3-compatible obligatoire en cluster** (`PNEX_STORAGE_BACKEND=s3`, `PNEX_S3_*` ; RustFS de référence, jamais MinIO) : un stockage `fs` local à un pod rend les artefacts firmware, médias et frames de stitch invisibles des autres pods. Le démarrage en cluster le vérifie (workstream E).

## 2. Arrêt propre : `terminationGracePeriodSeconds ≥ 40`

`App::on_shutdown` enchaîne, dans cet ordre :

1. **Drain du worker de flows** (D106) : `draining=true`, le contrôleur déplace les orgs, le runtime est coupé, le worker se désinscrit — jusqu'à `drain_timeout_ms` (20 s par défaut).
2. **Vidage des écrivains bufferisés** (`services/drain.rs`, borné à 10 s) : dernier lot du batcher télémétrie → OpenObserve et entrées en attente du journal notify. Leurs émetteurs vivent dans des statiques : le canal ne se fermait jamais, le « flush final » ne s'exécutait pas et chaque rolling update perdait le dernier lot de chaque pod.
3. **Libération des baux singleton** `task:*` (`services/singleton.rs::release_all`) : les sweeps (reaper de liveness, watchdog OTA, pruner vidéo, reconcile rétention O2) reprennent tout de suite sur un autre pod au lieu d'attendre l'expiration (3 périodes).

Total ≈ 30 s au pire, d'où **`terminationGracePeriodSeconds` ≥ 40** (k8s envoie SIGKILL au-delà).

## 3. Migrations au boot

`auto_migrate` (`PNEX_AUTO_MIGRATE`, vrai par défaut en production) faisait migrer **chaque pod** au démarrage, sans verrou : N pods démarrés ensemble exécutaient les mêmes migrations en parallèle (DDL en double, échecs aléatoires).

Désormais (`App::boot` → `services/db_lock.rs::migrate_under_lock`) : sur Postgres, la migration s'exécute **avant** `create_app` de loco, sur une connexion dédiée qui tient un **verrou advisory de session** (`pg_advisory_lock`, clé `PNEXMIGR`) ; les autres pods attendent puis ne trouvent plus rien à appliquer. L'`auto_migrate` de loco est ensuite désactivé pour ce boot. Sur sqlite (mono-pod), le chemin loco est inchangé. Alternative toujours possible : `PNEX_AUTO_MIGRATE=false` et un Job k8s `pnex-server db migrate` avant le rollout.

Une migration ne doit jamais être incompatible avec la version N-1 encore en service pendant le rollout (ajouts d'abord, suppressions dans une release ultérieure).

## 4. Pools de connexions

Réglages par défaut (production/développement/studio) :

| Clé | Défaut | Variable | Pourquoi |
|---|---|---|---|
| `database.connect_timeout` / `acquire_timeout` | 5 000 ms | `DB_CONNECT_TIMEOUT` / `DB_ACQUIRE_TIMEOUT` | attente max d'une connexion du pool (500 ms transformait chaque pic en 500) |
| `database.idle_timeout` | 300 000 ms | `DB_IDLE_TIMEOUT` | 500 ms fermait et rouvrait les connexions en permanence |
| `database.max_connections` | 10 | `DB_MAX_CONNECTIONS` | |
| `queue.max_connections` | 2 | `PNEX_QUEUE_MAX_CONNECTIONS` | pool propre de la queue loco (poll/claim des jobs uniquement) |

Dimensionnement : `pods × (database.max_connections + queue.max_connections)` doit rester **sous** `max_connections` de Postgres (moins la marge admin). Les verrous par org (§5) tiennent **une connexion supplémentaire** par deploy en cours. Au-delà de quelques dizaines de pods : pgbouncer en mode *session* (les verrous advisory de transaction et de session exigent qu'une transaction/session reste sur la même connexion serveur ; le mode *transaction* casse le verrou de migration).

## 5. Cohérence base sous concurrence

Principe : la base est la source de vérité ; aucune garde « lire puis écrire » ne repose sur un état en mémoire de pod.

| Chemin | Mécanisme |
|---|---|
| Deploy / rollback / start / stop / restart / delete de flow, arrêt auto au changement de mode d'un pin | **verrou advisory par org** (`FLOW_DEPLOY`, porté par une transaction dédiée pendant l'acquittement runtime) : garde d'exclusivité pins/caméras + application + marquage `deployed` atomiques par org. Attente bornée à 90 s → 503 `flow_runtime`. |
| Projection des régulations (`regulator_configs`) | recalcul **par org** (plus de réécriture de toutes les orgs à chaque deploy), lecture + calcul + réécriture dans **une** transaction sous verrou advisory par org ; versions et devices chargés en lot. |
| Scans des flows déployés (gardes, pinout, projection) | 2 requêtes (`flows` puis `flow_versions WHERE id IN (...)`) au lieu d'une par flow. |
| OTA | index unique partiel « une assignation active par device » (migration 000043) → 409 `ota_in_progress` ; transitions **compare-and-set** (`WHERE state = <attendu>`) : une notification terminale n'est émise que par l'appel qui a réellement fait la transition. |
| Sauvegardes versionnées (flows, tours, couches d'annotations, dashboards) | contrôle optimiste **dans** la transaction (verrou de ligne parent ; pointeur dashboard conditionnel `WHERE current_version_number = attendu`) ; violation d'unicité → 409 conflit, jamais 500 ; un restore concurrent n'est jamais écrasé. |
| Quotas (devices par type, builds firmware) | `COUNT` + insertion sous verrou advisory par org ; un build en file/en cours plus récent que l'intervalle minimal bloque aussi. |
| Présence device / anti-clone (D108) | **Valkey** : bail `pnex:{db}:live:lease:{device}` = session (PX = TTL de silence), claim / touch / release en scripts Lua compare-and-set (first-live-wins inter-pods, propriétaire vérifié) ; `last_seen` dans le ZSET `pnex:{db}:live:seen`. Postgres ne reçoit que les transitions (`active`, `device_states.last_seen_at` à la déconnexion et au retrait par le reaper). |
| Baux (`flow_leases` : contrôleur, `task:*`) | expiration calculée **et** comparée avec l'horloge de la base (`now()`), plus celle des pods. |
| Worker stitch 360 | claim conditionnel (`queued`, ou `running` périmé) : une double livraison est un no-op ; timeout par job (`PNEX_STITCH_TIMEOUT_SECS`, 1 500 s par défaut, < âge du reaper 30 min). |
| Purge O2 après suppression d'org | réessais avec backoff (5 s → 10 min) puis erreur journalisée avec l'org O2 pour purge manuelle (non durable à travers un redémarrage). |

## 6. Ce qui reste

- Heartbeats des workers de flows et `is_alive` comparés avec l'horloge des pods (dérive tolérée = TTL 10 s) ; seuls les baux sont passés à l'horloge base.
- Purge O2 post-suppression non durable (une table de purges en attente + sweep la rendrait rejouable).
- Le stitch bloquant (`spawn_blocking`) n'est pas interruptible : au timeout, le job passe `failed` mais le thread termine en arrière-plan.
- Upload concurrent des frames d'un job de stitch : `frames_received` est incrémenté en lecture-écriture (contrôleur hors périmètre du workstream D).

## 7. Présence device en Valkey (D108) — 2026-10-01

| # | Décision |
|---|---|
| **D108** | **Le bail de présence device quitte Postgres pour Valkey ; Valkey devient obligatoire.** Révise D9 (« que du PG »). Motif : 1 UPDATE `device_states` toutes les TTL/4 (2,5 s) par device connecté = ~400 écritures/s pour 1 000 devices (tuples morts, WAL, autovacuum) + reaper qui balayait toute la flotte toutes les 5 s. Désormais : bail = clé Valkey à expiration (scripts Lua claim / touch / release, même sémantique first-live-wins et propriétaire vérifié qu'en SQL), `last_seen` = ZSET ; Postgres ne garde que l'historique froid (`device_states.last_seen_at`, colonnes `connected` / `session_id` supprimées par la migration 000045) et le booléen `active` (reaper seul écrivain, écritures sur transition uniquement). Lectures API = fusion Valkey + Postgres (le plus frais gagne). Redémarrage Valkey : marqueur `epoch` perdu ⇒ le reaper suspend les désactivations pendant un TTL de silence (pas de bascule hors ligne de toute la flotte) ; pendant cette fenêtre un clone peut prendre un bail libéré — limite assumée, comme l'expiration du TTL. Clés namespacées par nom de base (`pnex:{db}:live:`). Les heartbeats des workers de flows (D106) restent en base : quelques lignes, liés transactionnellement aux placements. |
