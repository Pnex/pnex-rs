# PROGRESS — Migration PNEX Django → Rust (Loco + Dioxus)

> Pilotage : voir `migration.md`. Ne jamais commiter rouge (cargo check / dx build / cargo test).

## État courant

**Phase 0 — Inventaire & capture des contrats : TERMINÉE** (revue humaine validée le 2026-08-15).

- [x] Lecture de `migration.md`, cadrage
- [x] Exploration du repo Django `pnex-server` (6 axes : models, API DRF,
      WS/Channels/crypto, Celery/NATS/K8s/firmware, ETL/ES/metrics, auth/tenant)
      → rapports dans `docs/phase0/`
- [x] Rédaction `docs/inventory.md` (table maîtresse Django → cible Rust → phase)
- [x] Capture des contrats dans `docs/contracts/` (exemples .http + README parité)
- [x] Revue humaine de la Phase 0 — points ouverts tranchés (décisions D4-D11,
      voir `docs/inventory.md` §0 et §7)

**Phase 1 — Squelette du workspace : EN COURS** (branche `phase-1-squelette`).

- [x] Workspace Cargo 4 crates : `pnex-core` (serde pur, natif+wasm32),
      `pnex-backend` (Loco v1.0, bin `pnex-server`), `pnex-frontend`
      (Dioxus 0.7 CSR web), `pnex-firmware-builder` (stub Phase 6)
- [x] Backend minimal : `/health/live` + `/health/ready` (DB « unconfigured »
      jusqu'en Phase 2), front servi en statique (middleware static + fallback SPA)
- [x] Taskfile (`task check/test/build/dev/dev:hot/lint`), CI GitHub Actions
      (check natif+wasm32, test, dx build web)
- [x] Doc `docs/architecture/features.md` — piège fullstack/hydration documenté
- [x] Vérifié : `task check` vert (natif + wasm32), `task test` vert,
      `dx build --release` OK, serving bout-en-bout (live/ready/front/wasm 200)
- [x] Remote GitHub `Pnex/pnex-rs` ajouté (opensource) — push au fil de l'eau

**Divergences assumées vs Django** (Phase 1) : service renommé
`og-device-hub` → `pnex-server` ; health **sans slash terminal** ; `/health/ready`
sans DB (Phase 2 branchera le check PG, le « cache » deviendra OpenObserve).

**Prochaine : revue humaine Phase 1**, puis Phase 2 — Modèles & DB
(PostgreSQL, SeaORM, organisations D2, seed fixtures YAML).

## Décisions

| Date | Décision | Pourquoi |
|------|----------|----------|
| 2026-08-15 | Pas de 2e repo Web UI à réintégrer — le front Dioxus sera écrit from scratch | Confirmé par l'utilisateur (migration.md §1 mentionnait une Web UI séparée : obsolète) |
| 2026-08-15 | Phase 0 lancée sur le repo Django `/home/shan/Documents/shan-perso/pnex-server` | — |
| 2026-08-15 | **Fonctions utilisateur en 2 couches** : (1) ETL d'ingestion en **VRL dans les pipelines OpenObserve** (conversions, champs dérivés, routage) ; (2) fonctions complexes utilisateur en **WASM/wasmtime multi-langage** avec host functions — dont une host fn `coolprop()` qui appelle le service FastAPI côté hôte | VRL **ne supporte pas les appels HTTP** (limitation Vector [#22783](https://github.com/vectordotdev/vector/issues/22783)) → CoolProp injoignable depuis VRL ; enrichment tables CSV inadaptées à la thermo (T,P)-dépendante. Confirmé par recherche 2026-08-15, proposé par l'utilisateur, à valider en revue de phase |
| 2026-08-15 | **VRL abandonné — tout l'ETL en Rust/WASM dans le backend** : OpenObserve devient purement stockage + query + dashboards + reports (pas de pipelines VRL). L'ETL tourne dans Loco (l'ingestion passe déjà par le backend : WS ChaCha20 → Loco → OpenObserve). Deux niveaux côté moteur : (a) **évaluateur d'expressions sûr en Rust** (parité safe_eval Django : opérateurs/fonctions/constantes whitelistés) pour les formules/conversions existantes ; (b) **WASM/wasmtime multi-langage** pour les fonctions custom utilisateur, host functions dont `coolprop()` → FastAPI | Un seul moteur au lieu de deux (VRL + WASM) ; le backend est déjà dans le chemin de données donc VRL n'apporte rien ; testabilité cargo ; pas de compétences VRL niche à maintenir. Décidé par l'utilisateur 2026-08-15 |
| 2026-08-15 | **Multi-tenant : l'organisation est le tenant** — 1 org OpenObserve par org PNEX, et **une org peut contenir plusieurs users** (membership `user ↔ org` en PG, avec rôle). Tables : `organizations` + `organizations_members`. Le scoping des données (devices, formules, sites…) passe de `user_id` (Django) à `org_id`. Devices ne parlent JAMAIS directement à OpenObserve — ingestion via WS Loco, le backend écrit dans l'org avec un credential service | L'org est l'unité de tenue native d'OpenObserve (streams, rôles, **retention par org** = aligné sur les tiers d'abonnement Free 1 j → Ultimate 2 ans). Plusieurs users par org = besoin réel (équipes). ⚠️ Nouveau concept vs Django → impacte schéma PG (Phase 2) et API (Phase 3-4). **Validé en revue** |
| 2026-08-15 | **Rapports → OpenObserve Report Server** : rapports PDF = **scheduled reports de dashboards OpenObserve** (rendu PDF via Report Server, SMTP, cron). Supprime matplotlib + WeasyPrint + Celery generate_report + stockage S3 des rapports | `schedule {cron, email_to}` de ReportConfiguration mappe 1:1. Le layout JSON ReportTemplate était du **code mort**. Formula results déjà indexés dans OpenObserve (`source_type: "formula"`) |
| 2026-08-15 | **D13 — Actuateurs sans serveur au milieu (chantier M2M différé)** : les actionneurs **ingèrent leur propre config**, actuateurs ↔ capteurs **communiquent en direct**. Le serveur garde : stockage/édition des configs (API+UI) + capture/ETL. La mécanique de distribution (broadcast desired-state, push WS, protocole) est reportée à un chantier séparé — ne pas sur-concevoir Phases 4-5 | Vision edge complète confirmée par l'utilisateur ; ws/actuator/cast (config + state) marqué « différé » dans l'inventaire |
| 2026-08-15 | **Revue de phase — points tranchés (D4-D12)** : firmware sur **MinIO/S3 conservé** (abstraction `ArtifactStore`, PG écarté — binaires RTOS/OS complets trop lourds pour PG/backups/WAL) ; rétention artifacts = structure maintenant, gestion plus tard ; rapports = conception détaillée repoussée mais exigences verrouillées (**provisioning/cron OpenObserve par API** via service account, génération live en **tâche backend** anti-saturation) ; **ChaCha20 nu à parité** + versionnement protocole pour upgrade AEAD ultérieur ; **état live device → Postgres** (`device_state` upsert + purge TTL) ; **tokens DRF supprimés** (JWT Keycloak seul, DeviceToken inchangés) ; **abonnement attaché à l'org** ; **timestamps télémétrie** (D12) : fallback dt d'ingestion + provenance, protocole v2 avec timestamp optionnel, SNTP recommandé côté ESP32 | Revue humaine 2026-08-15 — l'utilisateur a validé l'ensemble et délégué les choix restants ; D12 ajouté suite à sa question sur les devices sans NTP |
| 2026-08-15 | **Renommage service** : `og-device-hub` (héritage Django) → `pnex-server` — l'ancien nom n'est plus utilisé | Demandé par l'utilisateur 2026-08-15 |
| 2026-08-15 | **Remote GitHub `Pnex/pnex-rs`** (opensource) ajouté — commits poussés au fil de l'eau | Demandé par l'utilisateur 2026-08-15 |
| 2026-08-15 | **Phase 1 technique** : scaffold Loco v1.0 (`--db none --bg async --assets clientside`) puis trim ; dx 0.7.10 n'a pas de flag `--project` (il faut `cd` dans le crate) et sort dans `target/dx/...` (le Taskfile copie vers `crates/frontend/dist`) ; assets via macro `asset!()` (manganis hash les fichiers), pas via `[web.resource].style` | Constaté à l'implémentation |

## Principes directeurs (confirmés par l'utilisateur)

- **Périmètre pnex-rust = partage de config + capture + ETL.** Rien d'autre.
  Pas d'anti-pattern non industrialisable ou peu robuste (ex : pod K8s par
  actuateur, fan-out Celery par user, contournements de bugs Argo).

## Journal

- 2026-08-15 : **Phase 1 — squelette du workspace implémenté** (branche
  `phase-1-squelette`) : 4 crates, Loco v1.0 minimal (health), Dioxus 0.7 CSR,
  Taskfile, CI, doc features. Chaîne vérifiée vert-de-vert (check natif+wasm,
  tests, build release, serving statique). En attente de revue humaine.
- 2026-08-15 : **Phase 0 terminée et validée**. Livrables : `docs/inventory.md`,
  `docs/phase0/` (6 rapports), `docs/contracts/` (exemples .http + règles de parité).
  Décisions D1-D11 consignées. Prochaine étape : Phase 1 (squelette workspace).
- 2026-08-15 : démarrage Phase 0. Workspace encore vide (uniquement migration.md).
  Repo source : `pnex-server` (Django 6, DRF, Channels, Celery, NATS, ES, K8s).
