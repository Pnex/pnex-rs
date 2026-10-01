# Administration système — rétention O2, nettoyage, statut plateforme (D72)

> Décision utilisateur 2026-09-28 : une page « Système » par organisation
> (rétention + nettoyage des données OpenObserve) et une page « Statut
> plateforme » réservée à l'administrateur.

## Rôles

| Rôle | Portée | Droits |
|---|---|---|
| Admin plateforme (`users.platform_admin`) | toute l'instance | statut plateforme, défaut global de rétention, override par org, test IA serveur |
| Owner / admin d'org | son org | nettoyage O2 (stream, plage, purge) |
| Viewer | son org | lecture seule (rétention effective, liste des streams) |

- Bootstrap : `PNEX_PLATFORM_ADMIN_EMAILS` (emails séparés par des virgules), lu au login — l'utilisateur listé est promu, **jamais rétrogradé** automatiquement (la base fait foi). Dev : `admin@example.com` (Taskfile `dev:backend`).
- Un admin plateforme n'est pas implicitement membre de toutes les orgs.

## Rétention effective

| Mode (`PNEX_DEPLOYMENT_MODE`) | Précédence |
|---|---|
| `self_hosted` (défaut) | override org → défaut plateforme (`system_settings.default_retention_days`) → `PNEX_DEFAULT_RETENTION_DAYS` (30) |
| `saas` | override org (admin plateforme, contrats spécifiques) → tier d'abonnement (`data_retention_secs` arrondi au jour supérieur) → env |

- O2 : rétention **par stream**, en jours entiers, **minimum 1 jour** — valeur bornée (le tier seedé « Admin » à 5 min donne 1 j ; l'UI signale une valeur relevée).
- Application : `PUT /api/{org}/streams/{name}/settings?type=metrics` `{"data_retention": N}` (vérifié O2 v1.0.0). Réconciliation **horaire** de tous les streams des orgs provisionnées (un stream naît à la première ingestion d'une métrique) + immédiate après chaque changement de réglage.
- `0` côté O2 = défaut de l'instance (`ZO_COMPACT_DATA_RETENTION_DAYS`) : affiché « défaut O2 » tant que la réconciliation n'est pas passée.

## Nettoyage O2

| Action | Endpoint PNeX | Appel O2 |
|---|---|---|
| Supprimer un stream | `DELETE /api/v1/system/o2/streams/{name}` | `DELETE /api/{org}/streams/{name}?type=metrics` |
| Supprimer une plage | `POST …/streams/{name}/delete-range` `{start,end}` RFC 3339 | `DELETE …/data_by_time_range?start&end` (µs) |
| Purger l'org | `POST /api/v1/system/o2/purge` `{confirm: "<nom org>"}` | suppression de chaque stream |

- Plage : O2 exige un **début aligné sur l'heure** → début arrondi à l'heure inférieure, fin à l'heure supérieure (la fenêtre supprimée couvre toujours la demande). Traitement **asynchrone** par le compacteur : la réponse vaut « planifiée ».
- Toutes les opérations de gestion passent en Basic root (le passcode d'ingestion n'a pas ces droits).
- Suppression d'une org PNeX : l'identifiant O2 est lu avant la cascade, puis ses streams sont purgés en tâche de fond (panne O2 = log, jamais de blocage).

## Statut plateforme

`GET /api/v1/system/status` — sondes parallèles, chacune bornée à 3 s ; statut `ok | degraded | down | not_configured`, métriques à clé machine (`system-metric-<clé>` côté UI), détails verbatim (diagnostic runtime).

| Composant | Sonde | Mesures |
|---|---|---|
| Base de données | `SELECT 1` | moteur, taille (`pg_database_size` / `page_count × page_size`), 5 plus grosses tables (PG) |
| Rauthy | `/auth/v1/health` (`db_healthy`, `cache_healthy`) + `/auth/v1/version` | version, issuer |
| OpenObserve | `/healthz` + streams de chaque org provisionnée | stockage brut/compressé, points, streams, top orgs |
| Valkey | `PING` + `INFO memory/server` | mémoire utilisée, version |
| Stockage objets | backends firmware/média ; S3 : `Operator::check()` opendal par bucket distinct | tailles via le catalogue DB (`SUM(size_bytes)`), **jamais de listing de bucket** |
| Stockage local | parcours borné (200 000 entrées, cache 5 min) des dossiers médias / flow-state / modèles stitch | taille par dossier (mention « partial » si borne atteinte) |
| Machine hôte | `statvfs` (/ + volume data), `/proc/meminfo`, `/proc/loadavg`, `/proc/uptime` ; type : `/proc/device-tree/model` (Raspberry Pi), DMI (VM), `/.dockerenv`/cgroup (conteneur) | disque, RAM, charge, uptime, arch, cœurs |
| Service IA | fournisseur par défaut de la plateforme (D116), fournisseurs d'org comptés | fournisseur, modèle, endpoint ; « non configuré » sans défaut plateforme. Les fournisseurs plateforme se gèrent sur la même page (`/api/v1/system/ai/providers`, test live par fournisseur, jamais au chargement : coût LLM) |
| Coffre de secrets | trousseau `PNEX_SECRETS_KEYS` + comptage `org_secrets` par `key_id` | clé d'écriture, nombre de clés, secrets stockés, secrets sous une ancienne clé ; dégradé si une ligne est sous une clé absente. Bouton « Rechiffrer les secrets » (`POST /api/v1/system/secrets/rekey`, secrets.md §16) |

## Vue admin de toutes les organisations

`GET /api/v1/system/orgs` (admin plateforme) : chaque organisation de l'instance avec abonnement, rétention effective (+ source), override éditable en ligne (`PUT …/retention/orgs/{id}`), stockage O2 (somme des tailles compressées, listings parallèles bornés à 3 s) et quota. Rendue en bas de `/admin/status`.

## Quotas de stockage par abonnement

- `subscription_tiers.max_telemetry_mb` (null = illimité) — migration 000034 avec **rétro-remplissage** des tiers seedés (Free 100 Mo, Basic 1 Go, Pro 10 Go, Enterprise 100 Go, Ultimate 500 Go, Admin illimité) : les conteneurs ne rejouent jamais le seed.
- Actif **en SaaS uniquement** ; usage = somme des `compressed_size` O2 de l'org.
- **Informatif** : jauge sur `/system` + alerte au dépassement ; la collecte n'est **jamais bloquée** (couper l'ingestion perdrait des mesures — à décider explicitement si besoin).

## Reste à faire

- Blocage (ou dégradation) de l'ingestion au dépassement de quota — décision produit.
- Test E2E matériel sur Raspberry Pi (détection `raspberry_pi`, disque SD).
