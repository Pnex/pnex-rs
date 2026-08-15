# PROGRESS — Migration PNEX Django → Rust (Loco + Dioxus)

> Pilotage : voir `migration.md`. Ne jamais commiter rouge (cargo check / dx build / cargo test).

## État courant

**Phase 0 — Inventaire & capture des contrats : TERMINÉE** (revue humaine validée le 2026-08-15).

**Phase 1 — Squelette du workspace : TERMINÉE** (merge sur `main`, CI verte).

**Phase 2 — Couche données : TERMINÉE** (merge sur `main`, CI verte —
inclut les corrections post-revue : refs `?` loco-rs pures et allègement
fluides).

- [x] `compose.yaml` : PostgreSQL 18-alpine + Keycloak 26.3 (start-dev),
      volume PG sur `/var/lib/postgresql` (convention PG 18+), `.env.example`
- [x] Loco branché SeaORM/PG : crate `pnex-migration`, feature `with-db`,
      sections `database` dans les configs, `cli::main::<App, Migrator>`
- [x] 5 migrations (orgs/users, devices, ETL, sites, firmware) :
      organizations + organization_members (D2), tier sur l'org (D11),
      scoping `org_id` partout (ex-user_id), sites PK UUID,
      metrics.\* et tables rapport SUPPRIMÉS (D3), `argo_wf_job_name` supprimé
- [x] **Modèle « sans copies »** (directive user) : entités standard
      (conversions, formules) = `org_id` NULL, fournies par l'app,
      partagées en lecture ; une org ne matérialise une ligne que si elle
      crée/personnalise. Tables Django `formula_imports`/`conversion_imports`
      (copie par user) **supprimées**
- [x] **Allègement fluides** (directive user) : catalogue de fluides
      supprimé de la base — le service FastAPI externe (CoolProp/RefProp)
      est la source de vérité, ses erreurs sont renvoyées telles quelles au
      client ; tables Django `fluid_catalogs`/`fluid_property_groups`
      supprimées, remplacées par `fluid_mixtures` (mélanges custom par org,
      org_id NOT NULL, composition JSONB)
- [x] Durations Django INTERVAL → colonnes `*_secs` (bigint)
- [x] Entities SeaORM générées (`cargo loco db entities`, stubs models/)
- [x] `/health/ready` branché sur un `SELECT 1` réel (ok/degraded, testé
      avec PG up et PG coupé)
- [x] Seed idempotent `cargo loco task seed` : fixtures YAML Django
      réutilisées telles quelles (5 types, 22 caps, 4 MCU + generic à la
      volée, 4 predefined, 6 tiers, 66 conversions, 39 formules ;
      global_id non-UUID → uuid5 DNS comme Django)
- [x] Test d'invariants de schéma (`migration/tests/schema_invariants.rs`) :
      scoping org NOT NULL, catalogue ETL nullable, tables de copie et
      catalogue fluides absents, actions ON DELETE
- [x] Quotas Free unifiés sur le tier fixture (3/1/0) — les 5/2/1 codés en
      dur dans les views Django étaient des fallbacks divergents
- [x] Taskfile : `db:up/down/migrate/entities/seed/reset` ; CI : service
      PostgreSQL 18 pour le job test

**Correction (post-revue)** : le « bug loco-rs 1.0.1 » était une erreur
d'usage. Le suffixe `?` se met sur le **nom de la table référencée**
(1er élément du tuple de refs) et la colonne ne doit **pas** être
redéclarée dans `cols` — sinon la déclaration écrase la colonne générée,
sans FK `SET NULL`. Migrations 000001/000003 réécrites avec les refs `?`
pures (plus de SQL brut pour les FK), test d'invariants étendu aux
actions ON DELETE (`SET NULL` sur nullable, `CASCADE` sur obligatoire).

**Phase 3 — Auth & multi-tenant : EN COURS** (branche
`phase-3-auth-multitenant`).

- [x] Realm Keycloak provisionné par fichier versionné
      (`deploy/keycloak/pnex-realm-realm.json`, import compose
      `--import-realm`) : client public `pnex` (PKCE S256 forcé, redirect
      localhost:*), users de test alice/bob. Recréation auto du conteneur =
      réimport.
- [x] Validation JWT locale par JWKS (`auth/jwks.rs`) : RS256, `iss` + `aud`
      explicites, exp, refresh sur `kid` inconnu — durcissements des failles
      Django n°3-4 (rapport Phase 0 §3). CORS inutile par conception (front
      same-origin servi par le backend) — faille n°1 réglée. Refus par
      défaut via extracteur `AuthUser` — faille n°2.
- [x] JIT provisioning transactionnel (`auth/provisioning.rs`) : users +
      user_profiles + org personnelle owner + tier Free, idempotent,
      re-vérification en transaction (concurrence), resync email/nom.
- [x] Extracteur `OrgContext` (`X-Org-Id` + membership) : point d'ancrage
      du scoping multi-tenant (remplace le filtrage par-viewset Django).
- [x] Proxy OAuth2 (`controllers/oauth2.rs`) : token (password +
      authorization_code+PKCE), refresh, sso 302 (`kc_action`), erreurs
      Keycloak relayées telles quelles.
- [x] `GET /api/v1/user-info` : identité + profil + orgs (rôle, tier) +
      device_count agrégé sur les orgs du user.
- [x] Endpoints organisations (`controllers/orgs.rs`) : CRUD + membres
      (ajout par email d'un user déjà provisionné, rôles lowercase,
      garde-fous : ≥1 owner, suppression = owner et dernier membre).
- [x] Tests : `auth_jwks.rs` (7 tests : iss/aud/exp/RS256/kid/malformé
      contre mock JWKS — pas de Keycloak en CI) et `tenant_isolation.rs`
      (4 tests HTTP : 401 sans token, JIT, isolation croisée alice/bob,
      rôles viewer/owner + garde-fous).
- [x] Configs : `settings.keycloak` (base_url/realm/client_id) dans les 3
      configs, `auto_migrate` en dev/test, hook `truncate` +
      `dangerously_truncate` en test (le create/drop de base loco bute sur
      le pool — workaround), `.env.example` complété.

**Front Phase 3 (port de `pnex-ui` React) : EN COURS** — directives user :
sélection de serveur **supprimée pour le web** (same-origin), URL serveur
auto-hébergée « façon Bitwarden » **uniquement pour desktop/ios/android**
(décision : architecturer seulement — cfg natif compilé, écran `ServerUrl`
écrit non routé, build desktop = phase explicite ultérieure) ; **i18n
obligatoire dès maintenant** (zéro libellé en dur, fr-FR + en-US) ; login =
**PKCE redirect** (pas de form password).

Commits faits (gates verts à chacun : `task check` natif+wasm32, `task test`,
`task lint`, `task build:frontend`) :

- [x] `Socle UI front : routeur Dioxus, Tailwind v4 et i18n fr-FR/en-US` —
      routes statiques + callback OAuth en query segments ; Tailwind v4 via
      `@tailwindcss/cli` (package.json dans le crate, `style/tailwind.css` →
      `assets/tailwind.css` généré gitignoré, tâches `css:build`/`css:watch`,
      step npm en CI) ; i18n Fluent (`dioxus-i18n`), locales embarquées,
      résolution localStorage > navigator.language > en-US ; stockage
      clé/valeur abstrait (web-sys localStorage/sessionStorage, mémoire en
      natif) ; CORS dev-only :5151 dans development.yaml (dx serve)
- [x] `DTO API phase 3 dans pnex-core` — TokenResponse, UserInfo/UserProfile,
      OrgMembership/TierInfo/DeviceCount, OrgSummary/OrgDetail/OrgMember,
      ProfilePatch, CreateOrg/UpdateOrg/AddMember/UpdateMember (rôles strings
      minuscules), tests de désérialisation sur les formes réelles
- [x] `Front : client HTTP, session, login PKCE, shell et toasts` — client
      reqwest (URLs relatives, Bearer + X-Org-Id, refresh 401 **single-flight**
      + retry unique, messages d'erreur relayés **tels quels** : detail >
      message > bloc error, 204→None) ; PKCE S256 (vecteur RFC 7636 testé),
      redirect `/api/v1/oauth2/sso` (action register/reset), callback
      `/auth/callback` → échange → user-info → session ; session globale
      Booting/LoggedOut/Authenticated (boot au start, langue du profil, org
      restaurée/validée) ; shell complet (sidebar `Layout.tsx` portée, garde
      de session = Login en place de l'Outlet, sélecteur d'org, toasts 5 s)
- [x] `Backend : PATCH /api/v1/profile` — patch partiel, language normalisée
      courte (en/fr), theme light/dark/auto, bornes = colonnes, 400 sur
      invalide, profil créé aux défauts si absent ; 2 tests HTTP

Constats techniques Dioxus 0.7.10 (vérifiés dans les sources, à retenir) :

- `dioxus-web` fournit le **WebHistory par défaut** au launch (pas de
  `RouterConfig::history()` — cette API n'existe pas en 0.7.10) ;
- mutation d'un `GlobalSignal` depuis une fn : méthode intrinsèque
  `with_mut(&self)` (les setters du trait Writable exigent `&mut`, impossible
  sur un static) ; `SESSION.cloned()` pour lecture réactive ;
- attribut SVG `viewBox` s'écrit `view_box` en rsx ;
- `spawn` n'exige pas `Send` → futurs reqwest wasm (`!Send`) OK ; le client
  vit en `thread_local` pour cette raison (la cible desktop devra rendre les
  futurs `Send` — noté features.md) ;
- `Link` ajoute `active_class` en plus de `class` (ordre CSS imprévisible) →
  classes actives/inactives = littéraux complets calculés côté Rust.

Reste front (ordre) :

- [ ] page Organisations : liste/création/sélection + détail (membres, rôles,
      rename, suppression, ajout par email) — `api/orgs.rs` déjà écrit
- [ ] Dashboard sur données réelles `user-info` (device_count, orgs, tier,
      by_type) + Profil (identité lecture, préférences PATCH, switcher FR/EN
      persistant, change password → sso?action=reset, logout)
- [ ] pages Devices/Builds/Catalog en empty-states « Phase 4/6 »
- [ ] test de parité des clés fr-FR/en-US (.ftl)
- [ ] docs : features.md (desktop = phase explicite, architecture préparée),
      convention.md (conventions front), ce fichier
- [ ] **e2e réel** (`task db:up && task dev` → :5150, Keycloak docker) :
      login alice → dashboard ; switcher FR/EN + reload + PATCH en base ;
      CRUD orgs avec bob (rôles, ≥1 owner, suppression, erreurs en toast) ;
      changement d'org → refetch X-Org-Id réseau ; access_token corrompu →
      refresh transparent ; refresh_token supprimé → session expirée ;
      deep links + back/forward ; logout ; `task dev:hot` :5151 (CORS dev) ;
      liens register/reset
- [ ] push + revue humaine Phase 3 (backend + front ensemble)

## Anciennes phases (détail)

### Phase 1 — Squelette du workspace (TERMINÉE, merge `main`)

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

**Positionnement** (2026-08-15) : Django était un POC jamais passé en prod —
**la version Rust devient la version officielle**. Pas de parité cosmétique
(slashs terminaux, etc.) ; seuls les contrats fonctionnels Phase 0
(`docs/contracts/`) doivent être conservés. Conventions centralisées dans
`convention.md`. `/health/ready` reste honnête sans DB jusqu'en Phase 2.

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
| 2026-08-15 | **Phase 1 technique** : scaffold Loco v1.0 (`--db none --bg async --assets clientside`) puis trim ; dx 0.7.10 n'a pas de flag `--project` (il faut `cd` dans le crate) et sort dans `target/dx/...` (le Taskfile copie vers `crates/pnex-frontend/dist`) ; assets via macro `asset!()` (manganis hash les fichiers), pas via `[web.resource].style` | Constaté à l'implémentation |
| 2026-08-15 | **Rust = version officielle** (Django = POC jamais en prod) : divergences cosmétiques non à justifier, contrats fonctionnels Phase 0 conservés ; conventions (noms, chemins, git, API) centralisées dans `convention.md` ; répertoires des crates renommés `crates/pnex-*` pour correspondre aux noms de packages | Demandé par l'utilisateur 2026-08-15 |
| 2026-08-15 | **Phase 2 — modèle « sans copies »** : fonctions/conversions **standard = fournies par l'app** (`org_id` NULL, partagées en lecture) ; fonctions **user = par org** (ligne matérialisée seulement si l'org crée/personnalise). Tables Django `formula_imports`/`conversion_imports` (copie par user + suivi de mise à jour) supprimées | Demandé par l'utilisateur 2026-08-15 (« éviter de faire des copies à chaque org/utilisateur ») |
| 2026-08-15 | **Phase 2 — fluides hors base** : catalogue de fluides supprimé de la DB — le service FastAPI externe (CoolProp/RefProp) est la source de vérité et ses **messages d'erreur sont renvoyés tels quels au client** ; la base ne garde que les **mélanges custom par org** (`fluid_mixtures`, composition JSONB) ; `fluid_property_groups` (config app) passe en code Rust | Demandé par l'utilisateur 2026-08-15 (« on va gérer tout côté service FastAPI refprop… alléger la base, sauf mélanges custom ») |
| 2026-08-15 | **Phase 2 technique** : Durations Django INTERVAL → `*_secs` bigint ; quotas Free unifiés sur 3/1/0 (tier fixture — les 5/2/1 des views Django étaient des fallbacks divergents) ; `global_id` non-UUID → uuid5 DNS (parité bootstrap_db Django) ; refs `?` nullable de loco-rs : le « bug » était un mauvais usage (`?` sur le 1er élément, pas de colonne dans `cols`) — corrigé, FK SET NULL via refs pures | Constaté à l'implémentation |

## Principes directeurs (confirmés par l'utilisateur)

- **Périmètre pnex-rust = partage de config + capture + ETL.** Rien d'autre.
  Pas d'anti-pattern non industrialisable ou peu robuste (ex : pod K8s par
  actuateur, fan-out Celery par user, contournements de bugs Argo).

## Journal

- 2026-08-15 : **Phase 3 — auth & multi-tenant implémentée** (branche
  `phase-3-auth-multitenant`) : realm Keycloak versionné, validation JWKS
  durcie, JIT provisioning (user + profil + org owner/Free), extracteurs
  AuthUser/OrgContext, proxy OAuth2, user-info, CRUD orgs+membres, 11 tests
  (JWKS mock + isolation tenant HTTP). Vérifié bout-en-bout contre Keycloak
  réel (alice/bob). En attente de revue humaine.
- 2026-08-15 : **Phase 2 — couche données implémentée** (branche
  `phase-2-modeles-db`) : compose PG 18 + Keycloak 26.3, SeaORM branché,
  5 migrations, 27 entities, modèle sans copies, /health/ready réel,
  seed idempotent (fixtures Django), test d'invariants, tâches Taskfile db:*,
  CI avec service PG. En attente de revue humaine.
- 2026-08-15 : **Nettoyage Phase 1 (noms & positionnement)** : répertoires des
  crates renommés `crates/pnex-*` (= noms de packages), toutes les références
  alignées (Cargo.toml, Taskfile, CI, configs Loco, .gitignore, doc) ;
  `convention.md` créé (noms, chemins, git, API, langue) ; repositionnement
  « Rust = version officielle, Django = POC » — slashs terminaux Django abandonnés.
- 2026-08-15 : **Phase 1 — squelette du workspace implémenté** (branche
  `phase-1-squelette`) : 4 crates, Loco v1.0 minimal (health), Dioxus 0.7 CSR,
  Taskfile, CI, doc features. Chaîne vérifiée vert-de-vert (check natif+wasm,
  tests, build release, serving statique). En attente de revue humaine.
- 2026-08-15 : **Phase 0 terminée et validée**. Livrables : `docs/inventory.md`,
  `docs/phase0/` (6 rapports), `docs/contracts/` (exemples .http + règles de parité).
  Décisions D1-D11 consignées. Prochaine étape : Phase 1 (squelette workspace).
- 2026-08-15 : démarrage Phase 0. Workspace encore vide (uniquement migration.md).
  Repo source : `pnex-server` (Django 6, DRF, Channels, Celery, NATS, ES, K8s).
