# Migrations de schéma — règles à partir de la première release

> Date : 2026-10-01 (première release). Décision D120.

## 1. Point de départ : une base unique

Les 50 migrations de la phase de développement (août → octobre 2026) sont
remplacées par **une seule migration de base**,
`m20261001_000001_baseline`, qui exécute un script SQL par moteur :

- `crates/pnex-backend/migration/src/baseline/postgres.sql`
- `crates/pnex-backend/migration/src/baseline/sqlite.sql`
- `crates/pnex-backend/migration/src/baseline/tables.txt` (ordre de
  création, utilisé par `down()`)

Le script PG a été généré depuis la chaîne historique (`pg_dump
--schema-only`) puis nettoyé ; le script SQLite a été réaligné sur PG. Au
passage :

- reliquats pré-coffre supprimés : table `ai_connectors`, colonne
  `wifi_credentials.wifi_password`, reprises au boot (`takeover_at_boot`
  et les 4 `takeover` notify/flow/wifi/llm), champ en clair des jobs de
  build ;
- `llm_providers.org_id` NOT NULL (D119 : pas de LLM plateforme) ;
- **dérive SQLite corrigée** : 11 tables mortes que seul PG supprimait
  (`sites`, `buildings`, `viz_links`…), `map_pins.site_id` NOT NULL vers
  une table disparue (création de POI impossible en SQLite), colonnes
  `annotation_layers.media_asset_id/tour_id` et `map_pins.preview_*`
  manquantes, 7 clés étrangères ajoutées par `ALTER` jamais propagées.

Toute base créée avant la release doit être **recréée** (`task db:reset`) ;
les données OpenObserve et RustFS ne sont pas concernées.

## 2. Règles pour chaque nouvelle migration

1. **Un fichier par changement**, nommé `mAAAAMMJJ_NNNNNN_sujet.rs`,
   numéro strictement croissant après `000001`. Jamais de modification
   d'une migration déjà publiée : on corrige par une nouvelle migration.
2. **Additive d'abord (expand/contract).** Ajouter une colonne nullable
   ou avec défaut, une table, un index : une migration. Retirer ou
   renommer : en deux releases — (a) le code cesse de lire/écrire
   l'ancien élément, (b) une migration ultérieure le supprime.
3. **Les deux moteurs, toujours.** Chaque `up()` est écrit pour
   PostgreSQL **et** SQLite. SQLite ne sait ni ajouter une FK ni modifier
   une colonne par `ALTER` : reconstruire la table (créer la nouvelle,
   copier, supprimer, renommer) plutôt que de sauter l'étape sur SQLite.
   Le test `migration/tests/schema_parity.rs` (tables, colonnes, FK
   identiques sur les deux moteurs) est **bloquant**.
4. **Énumérations.** Les 8 types `ENUM` PG de la base sont conservés ;
   ajouter une valeur = `ALTER TYPE … ADD VALUE IF NOT EXISTS` (PG) et
   rien en SQLite (texte). Pour une **nouvelle** énumération, préférer une
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
   de la base, parité PG/SQLite).

## 3. Ouvertures prévues

- `org_secrets.org_id` reste nullable (NULL = secret plateforme) : un
  futur secret de plateforme (SMTP, relais de notification) s'y range sans
  migration.
- Les numéros de verrous consultatifs déjà utilisés (`db_lock::ns`) ne sont
  jamais réattribués, même retirés.
