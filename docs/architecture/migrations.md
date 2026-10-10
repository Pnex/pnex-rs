# Migrations de schéma — règles à partir de la première release

> Date : 2026-10-01 (première coupe, D120) ; **recoupée le 2026-10-09** :
> la 0.1.0 n'est pas publiée (bêta), la base est refaite jusqu'à elle.
> **PostgreSQL seul depuis le 2026-10-10** (décision #19, §4).

## 0. Recoupe du 2026-10-09 (avant 0.1.0)

Les 6 migrations ajoutées depuis la première coupe (`controls`, `origin`,
conversations de l'assistant, rétention IA d'org, raison d'échec de build,
PKI devices) sont **fondues dans la base**, renommée
`m20261009_000001_baseline`. Au passage, colonnes de compatibilité
supprimées : `device_registries.metadata` (et la route `PUT/PATCH
/devices/{id}`), `pnex_hosts.ws_ssl` (toujours wss), `build_records.success`
(doublon de `build_phase`, désormais NOT NULL) ; `device_tokens.encryption_key`
passe NOT NULL (clé Noise obligatoire).

Une base créée avant est **refusée au démarrage** (ses migrations
appliquées n'existent plus) : `task db:reset`. Tant que la 0.1.0 n'est pas
publiée, une rupture de schéma se fait dans la base, pas par migration ;
les règles du §2 s'appliquent à partir de la 0.1.0.

## 1. Point de départ : une base unique

Les 50 migrations de la phase de développement (août → octobre 2026) sont
remplacées par **une seule migration de base** (aujourd'hui
`m20261009_000001_baseline`, voir §0), qui exécute un script SQL
PostgreSQL :

- `crates/pnex-backend/migration/src/baseline/postgres.sql`
- `crates/pnex-backend/migration/src/baseline/tables.txt` (ordre de
  création, utilisé par `down()`)

Le script PG a été généré depuis la chaîne historique (`pg_dump
--schema-only`) puis nettoyé. Au passage :

- reliquats pré-coffre supprimés : table `ai_connectors`, colonne
  `wifi_credentials.wifi_password`, reprises au boot (`takeover_at_boot`
  et les 4 `takeover` notify/flow/wifi/llm), champ en clair des jobs de
  build ;
- `llm_providers.org_id` NOT NULL (D119 : pas de LLM plateforme) ;
- le script SQLite de l'époque (dérive corrigée) a été retiré le
  2026-10-10 avec tout le support SQLite (décision #19).

Toute base créée avant la release doit être **recréée** (`task db:reset`) ;
les données OpenObserve et RustFS ne sont pas concernées.

## 2. Règles pour chaque nouvelle migration

1. **Un fichier par changement** (à partir de la 0.1.0), nommé
   `mAAAAMMJJ_NNNNNN_sujet.rs`, numéro strictement croissant après `000001`. Jamais de modification
   d'une migration déjà publiée : on corrige par une nouvelle migration.
2. **Additive d'abord (expand/contract).** Ajouter une colonne nullable
   ou avec défaut, une table, un index : une migration. Retirer ou
   renommer : en deux releases — (a) le code cesse de lire/écrire
   l'ancien élément, (b) une migration ultérieure le supprime.
3. **PostgreSQL seul** (décision #19 de `roadmap.md`, 2026-10-10) :
   SQL PostgreSQL natif (JSONB, `ILIKE`, FK par `ALTER`, PostGIS à
   venir) ; aucune branche par moteur. Le script SQLite, le test de
   parité et le smoke test SQLite ont été retirés le 2026-10-10.
4. **Énumérations.** Les 8 types `ENUM` PG de la base sont conservés ;
   ajouter une valeur = `ALTER TYPE … ADD VALUE IF NOT EXISTS`. Pour une **nouvelle** énumération, préférer une
   colonne `varchar` + validation applicative : aucune migration pour
   ajouter une valeur.
5. **Conventions.** Tables au pluriel, `org_id bigint NOT NULL` + FK
   `ON DELETE CASCADE` pour toute donnée d'org (D2), `created_at` /
   `updated_at timestamptz DEFAULT CURRENT_TIMESTAMP NOT NULL`, index
   nommés `idx_<table>_<colonnes>`, uniques `uniq_<table>_<colonnes>`, FK
   `fk-<table>-<colonne>`.
6. **Données.** Une migration de schéma n'embarque pas de données
   métier : le catalogue est semé au boot (`PNEX_AUTO_SEED`). Une reprise
   de données (backfill) va dans la migration qui en a besoin, idempotente.
7. **`down()`** reste fourni pour les migrations additives ; une
   migration destructive documente qu'elle n'est pas réversible.
8. Après la migration : `task db:entities` (ou édition à la main des
   entités), puis `cargo test -p pnex-migration` (invariants, aller-retour
   de la base).

## 3. Ouvertures prévues

- `org_secrets.org_id` reste nullable (NULL = secret plateforme) : un
  futur secret de plateforme (SMTP, relais de notification) s'y range sans
  migration.
- Les numéros de verrous consultatifs déjà utilisés (`db_lock::ns`) ne sont
  jamais réattribués, même retirés.

## 4. PostgreSQL, moteur unique (décision #19, 2026-10-10)

PNEX ne parle plus qu'à PostgreSQL (image de référence : `postgres:18-alpine`
dans `compose.yaml`). `DATABASE_URL` doit pointer sur PostgreSQL ; une URL
`sqlite://` n'est plus prise en charge et **aucun chemin de migration**
n'existe depuis une base SQLite (recréer la base sur PostgreSQL). La file
de jobs Loco vit dans la même base (`pg_loco_queue`, `queue.kind:
Postgres` en dur dans les yaml).

### 4.1 Ce qui a été retiré

| Élément | Où | Remplacé par |
|---|---|---|
| Script `baseline/sqlite.sql` + test `schema_parity.rs` | `migration/` | `postgres.sql` seul ; `up()` refuse tout autre moteur |
| Smoke test `tests/sqlite_smoke.rs` | backend | couverture des tests d'intégration PG |
| Boot sur fichier SQLite temporaire (`meta`, `catalog_seed`, `seed_backfill`) | tests | base de test PG (`TEST_DATABASE_URL`) |
| `truncate` par `DELETE` + `PRAGMA defer_foreign_keys` | `app.rs` | `TRUNCATE … RESTART IDENTITY CASCADE` |
| Verrous consultatifs « no-op hors Postgres » | `services/db_lock.rs`, `tasks/seed.rs` | toujours pris ; `migrate_under_lock` ne renvoie plus de booléen |
| Baux `flow_leases` sur horloge des pods | `flow_cluster/store.rs` | horloge de la base (`now()`) et `ON CONFLICT DO NOTHING` |
| Repli « scan Rust » des labels effectifs | `resources/labels.rs` | CTE récursives + JSONB (GIN) |
| Filtre JSONB conditionnel des annotations | `controllers/annotation_layers.rs` | `@>` systématique |
| Taille de base par `PRAGMA page_count` | `system_status` | `pg_database_size` |
| Suppression manuelle des révisions avant le projet | `controllers/firmware_projects.rs` | cascade de la FK `firmware_project_id` |
| Tier « hobbyiste » SQLite | configs, `.env.example`, README, `firmware-build.md` | deux tiers : `postgres` et `s3` |

`sqlx-sqlite` reste compilé **transitivement** (feature `with-db` de
`loco-rs`) : aucune dépendance directe ne l'active, ne pas l'utiliser.

### 4.2 Ce que l'on s'autorise désormais

- SQL PostgreSQL natif dans les migrations comme dans les requêtes :
  JSONB et ses opérateurs (`@>`, `?`), index GIN/GiST, `WITH RECURSIVE`,
  `ON CONFLICT`, `ILIKE`, fonctions de date côté base (`now()`,
  `make_interval`).
- Clés étrangères ajoutées par `ALTER TABLE` (plus de reconstruction de
  table), y compris les FK circulaires (`published_version_id`,
  `deployed_version_id`, `current_version_id`).
- Verrous consultatifs (`db_lock::xact_lock`, `TenantLock`) sans garde de
  moteur ; un nouvel espace de verrou = une constante dans `db_lock::ns`
  (§3 : jamais réattribuée).
- Extensions (PostGIS envisagé pour le geofencing, `roadmap.md` axe G) :
  une extension est activée par migration (`CREATE EXTENSION IF NOT
  EXISTS`) et sa licence passe la règle des briques permissives ou une
  exception consignée.
- `PgExpr::ilike` n'est plus un piège (il paniquait sur le builder
  SQLite). Toute requête brute reste **paramétrée** (`$1`…), jamais de
  valeur utilisateur concaténée.

### 4.3 Tests

- Tous les tests base passent par PostgreSQL : `TEST_DATABASE_URL`
  (défaut `postgres://pnex:pnex@localhost:5432/pnex_test`, `config/test.yaml`).
- Chaque boot de test vide la base (`dangerously_truncate`) : un test ne
  suppose jamais une base vierge **autre** que celle qu'il vient de booter,
  et les tests qui bootent l'app restent `#[serial]`.
- Tests unitaires du crate qui ont besoin d'un schéma :
  `services::artifact_store::tests::migrated_test_db()` (migration sous
  verrou consultatif, sûre en parallèle).
- Worktree ou session parallèle : base dédiée (`CREATE DATABASE
  pnex_test_<nom> OWNER pnex`) puis `TEST_DATABASE_URL` vers elle ; une
  migration non commitée d'une autre session casse sinon tous les tests.

### 4.4 Dette héritée de la portabilité (simplifications possibles)

Le code ne porte plus de branche par moteur, mais certaines formes
« portables » restent et peuvent être simplifiées au fil de l'eau :

- recherche (`controllers/global_search.rs`, `controllers/pagination.rs`) :
  `lower(col) LIKE … ESCAPE '\'` avec motif abaissé en Rust peut devenir
  un `ILIKE` (même résultat sur PG, une fonction par colonne en moins) ;
- `services/dashboard.rs::build_stats` : réduction en Rust au lieu d'un
  `GROUP BY` ;
- intégrité « portée par le contrôleur » sur les FK circulaires : la FK
  existe en base, les vérifications applicatives restent comme garde de
  message d'erreur (409/404 propres), pas comme seule protection.
