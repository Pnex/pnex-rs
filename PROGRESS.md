# PROGRESS — Migration PNEX Django → Rust (Loco + Dioxus)

> Pilotage : voir `migration.md`. Ne jamais commiter rouge (cargo check / dx build / cargo test).

## État courant

**Phase 0 — Inventaire & capture des contrats : TERMINÉE** (revue humaine validée le 2026-08-15).

**Phase 1 — Squelette du workspace : TERMINÉE** (merge sur `main`, CI verte).

**Phase 2 — Couche données : TERMINÉE** (merge sur `main`, CI verte —
inclut les corrections post-revue : refs `?` loco-rs pures et allègement
fluides).

**Phase 4 — Devices (CRUD + catalogue + pagination D14) : TERMINÉE**
(merge sur `main` le 2026-08-16, revue utilisateur — CRUD devices puis
pagination validés ; gates verts au merge : check natif+wasm32, 48 tests,
clippy -D warnings, build front). **Périmètre réduit à la demande de
l'utilisateur : sensors uniquement — `actuator-channels` (backend comme
DTO) est différé** pour réfléchir avec le chantier M2M (D13) ; la
table/migration n'existe donc pas encore.

- [x] DTO `pnex-core::devices` (registre org, catalogue) — org_id à la
      place du `user` Django, dates en RFC 3339, tests roundtrip
- [x] Backend `controllers/devices.rs` : CRUD `/api/v1/devices`
      (réactivation implicite 200 vs 400 device actif, quotas tier par
      type — inactifs comptés, création inactive + DeviceToken auto en
      transaction : token urlsafe 32 o + clé ChaCha20 base64, update
      metadata-only 400 exact, DELETE nettoie build_records + token),
      catalogue authentifié `/api/v1/device-capabilities` + `/api/v1/
      predefined-devices` (filtre capabilities multi OU)
- [x] Durcissements documentés (vs Django POC) : écriture owner/admin,
      catalogue authentifié, 204 sans body (le body sur 204 de Django
      était illisible côté navigateur), filtre `revision` fonctionnel
      (Django filtrait `version=` inexistant → 500)
- [x] 8 tests de parité HTTP (mock JWKS) : cycle création/réactivation/
      refus, filtres, metadata-only, quotas 3/1/0, isolation tenant,
      rôles, suppression, catalogue partagé
- [x] Front : page Devices (filtres type/statut/capacité + recherche,
      enregistrement depuis le catalogue, détail avec token masqué +
      éditeur JSON metadata, suppression confirmée ; écriture masquée
      viewer), dashboard quotas « x / max » réels, i18n fr/en
- [x] **Pagination + recherche sur toutes les listes (D14)** : enveloppe
      unique `{count, next, previous, results}` (forme LimitOffset DRF)
      sur devices, predefined-devices, device-capabilities, orgs et
      membres ; `limit` (défaut var `PAGINATION_DEFAULT_LIMIT` à 10,
      max 100) / `offset`, invalide → défauts silencieux ; `search` OU
      insensible à la casse multi-champs (ILIKE + sous-requêtes SQL côté
      catalogue, filtre Rust puis découpage sur les ensembles bornés par
      quotas). Front : composant Pager + champ recherche (debounce
      gloo-timers — futures-timer panique sur wasm32) sur
      Devices/Catalogue/Organisations. Test `pagination_des_listes` +
      roundtrip de l'enveloppe dans le core
- [x] Doc conception build firmware (`docs/architecture/firmware-build.md`,
      Phase 6) : architecture cible du worker + contraintes vérifiées du
      dépôt `pnex-firmwares` (config device en vars d'env de `pio run`,
      HOST/TOKEN/DEVICE_ID en base64)


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

**Phase 3 — Auth & multi-tenant : TERMINÉE** (merge sur `main` le
2026-08-16, revue utilisateur — go donné après e2e curl ; gates verts :
check natif+wasm32, 30 tests, clippy -D warnings).

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

**Front Phase 3 (port de `pnex-ui` React) : TERMINÉ** — directives user :
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
  classes actives/inactives = littéraux complets calculés côté Rust ;
- **reqwest exige des URLs absolues, même en wasm** : `Url::parse` refuse
  les chemins seuls (RelativeUrlWithoutBase → « builder error » au premier
  appel, l'échange PKCE du callback). `api_base()` résout donc l'origine de
  la page (`location.origin`) quand `PNEX_API_BASE_URL` n'est pas fixé à la
  compilation — same-origin conservé, URLs absolues ;
- **Provisioning : re-liaison par email quand le `sub` Keycloak change**
  (realm réimporté, migration d'IdP) — sinon l'INSERT violait
  `users_email_key` (unique) → 500 sur user-info. Test
  `sub_change_avec_meme_email_relie_le_meme_user`. En contre-mesure dev,
  alice/bob ont des UUIDs pinés dans le realm (une recréation du conteneur
  Keycloak ne churne plus les identités) ;
- **Logout = end-session Keycloak** (RP-initiated logout) : le front purge
  ses tokens puis redirige en pleine page vers le proxy
  `/api/v1/oauth2/logout` (id_token_hint + post_logout_redirect_uri) —
  sinon le cookie SSO survit et le login suivant ré-authentifie sans
  formulaire. L'id_token est stocké côté front (scope openid envoyé par le
  proxy token). ⚠️ **Keycloak sépare les valeurs de
  `post.logout.redirect.uris` par `##`** (espace = pattern unique invalide
  → « Invalid redirect uri ») ;
- **Keycloak : `kc_action=register` sur l'authorize est ignoré quand une
  session SSO existe** (re-login silencieux au lieu du formulaire
  d'inscription). Le proxy SSO utilise l'endpoint registrations dédié pour
  `action=register` (kc_action=UPDATE_PASSWORD conservé pour reset) — test
  de non-régression sur les deux Location ;
- **Keycloak : les jokers de `redirectUris` ne sont valides qu'en FIN
  d'URI** — `http://localhost:*/*` (joker sur le port) ne matche rien →
  « Invalid parameter: redirect_uri » au login. Realm corrigé en URIs
  explicites (5150 backend, 5151 dev hot, localhost + 127.0.0.1).

Suite du front (terminée) :

- [x] page Organisations : liste/création/sélection + détail (membres, rôles,
      rename, suppression, ajout par email), garde-fous relayés en toasts
- [x] Dashboard sur données réelles `user-info` (device_count, orgs, tier,
      capacités) + Profil (identité lecture, préférences PATCH, switcher
      FR/EN appliqué immédiatement + persistant, change password
      → sso?action=reset, logout)
- [x] pages Devices/Builds/Catalog en empty-states « Phase 4/6 »
- [x] test de parité des clés fr-FR/en-US (fluent-syntax, 9 tests front)
- [x] docs : features.md (desktop = phase explicite, architecture préparée,
      contrainte Send pour la couche HTTP), convention.md (section Front),
      ce fichier

**Clôture Phase 3** : le parcours navigateur humain (login PKCE réel,
switcher, toasts) a été validé au fil des correctifs post-e2e (logout
end-session, register, re-liaison sub) ; l'utilisateur a donné le go au
merge le 2026-08-16.

E2E vérifié en curl (backend Loco :5150 + front buildé servi + Keycloak
docker) : serving SPA (index, tailwind, wasm, fallback /auth/callback),
401 sans token, proxy sso 302 PKCE vers Keycloak réel, password grant via
proxy → JIT → user-info, PATCH profile, orgs CRUD.

**Phase 5 (tranche 1) — Ingestion télémétrie collecte : TERMINÉE** (merge
sur `main` le 2026-08-16, revue utilisateur — go après e2e réelle vécue
ensemble : metrics O2, anti-clone, reaper, normalisation D16 ; gates verts
au merge : check natif+wasm32, 64 tests, clippy -D warnings, build front).
Périmètre à la demande de l'utilisateur : **collecte uniquement** —
broadcast desired-state et tout l'actuateur restent au chantier M2M (D13).
La suite de la Phase 5 (lecture métrics front, `ws/metrics/live`) reste à
faire et sera re-planifiée après la Phase 6.

- [x] Migration 000006 : `device_states` (bail de vie D9 : last_seen,
      connected — remplace Redis db2 Django) + `openobserve_orgs`
      (correspondance org PNEX ↔ org O2 + token d'ingestion correlé)
- [x] WS `/ws/sensor/ingest` (`controllers/ws_ingest.rs`, contrat
      `docs/contracts/ws-sensor-ingest.md`) : parité SensorIngest Django —
      auth b64 query (+trim du `\n` firmware), frames
      base64(nonce‖ChaCha20-nu), PING/PONG, key=value, validation
      stricte/découverte plafonnée, erreurs chiffrées, close codes
      4001-4008 ; durcissements : cache revalidation 10 s (§7.8 — Django
      requêtait la DB à chaque frame à ~10 fps), last_seen sur toute frame
      valide, déconnexion propre = bail libéré immédiatement
- [x] **Anti-clone (décision user, D15)** : sessions en-process → 4003
      immédiat + fallback `device_states` frais (crash/autre process) ;
      reaper 5 s seul écrivain de `active` (parité Celery Django),
      TTL silence 10 s configurable ; limite assumée : first-live-wins,
      un clone peut prendre la place après TTL
- [x] **OpenObserve metrics (D15)** : batcher 500/10 s → Prometheus
      remote-write `/api/{org}/prometheus/api/v1/write` (prost+snappy —
      demande user : les données vont dans les **metrics**, pas les logs),
      séries `metric{device_id, pred_dev, source_type, ts_source}` ;
      provisioning paresseux idempotent (org `pnex_org_{id}` cherchée par
      nom avant création — O2 v0.92.1 ne dédoublonne pas ; user d'ingestion
      admin + passcode, password réinitialisable par root si ligne PG
      perdue) ; compose `openobserve` v0.92.1 (port 5080) ;
      `/health/ready` check O2 (not-configured si absent)
- [x] Reaper déplacé dans `after_routes` : `loco start` sans flag est
      ServerOnly (connect_workers jamais appelé — constaté en e2e)
- [x] DTO devices + `last_seen` ; page Devices : « vu à HH:MM:SS » /
      « jamais vu » sous le badge de statut, i18n fr/en
- [x] **Normalisation des noms de mesures (D16)** : canonisation avant
      validation/découverte/stockage (accents, casse, séparateurs fondus) —
      `Soil-Moisture` ≡ `soil_moisture`, fini les rejets cosmétiques ;
      le nom de série O2 est canonique
- [x] Exemple `ingest_client` (rôle firmware chiffré, gère les close
      frames) ; 9 tests WS + 3 tests mock O2 fidèle + tests unitaires
      (crypto, fraîcheur, promwrite, mots de passe) — 63 au workspace
- [x] E2E réelle vérifiée (PG + Keycloak + O2) : création device via API
      → client chiffré → org O2 auto-provisionnée → données visibles via
      `/prometheus/api/v1/query` ; clone → close 4003 ; reconnect
      immédiat après déconnexion propre ; reaper active=true/false

**Phase 6 — Worker de build firmware : IMPLÉMENTÉE** (branche
`phase-6-firmware-builder`, en attente de revue humaine). Périmètre :
job asynchrone queue PostgreSQL → worker Loco subprocess PlatformIO →
artefact `.bin` téléchargeable. **Décisions user de la phase** :
`ArtifactStore` à deux backends — `local` (FS, edge) **d'abord**, `s3`
différé, sélection `STORAGE_BACKEND` (révision D5) ; suivi des builds par
**polling** front (WS firmware builds différé) ; front complet
(l'enregistrement d'un device collecte URL serveur + SSID + mdp WiFi et
déclenche le build — directive firmware-build.md §3).

- [x] Crate `pnex-firmware-builder` : trait `ArtifactStore`
      (put/get/delete/exists) + `LocalStore` (sanitize anti-traversal,
      clés D6 `org_{id}/firmware/{device_id}-firmware.bin`) + `S3Store`
      plomberie différée (`NotImplemented`) ; pipeline `run_build`
      (workspace TempDir effacé au drop — secrets compilés dans les
      artefacts intermédiaires ; source copie locale préservant le layout
      `common_libs/` ou git clone `--depth 1` ; `pio run` avec **env
      réduite** PATH/HOME/PLATFORMIO_CORE_DIR + config device — WiFi
      clair, HOST/TOKEN/DEVICE_ID base64, jamais en argv ; merge-bin
      esptool par SoC, esp8266 = image unique sans merge ; deadline
      globale + kill_on_drop) — 15 tests unitaires
- [x] Queue + worker : feature loco `worker`, `pg_loco_queue` (SKIP LOCKED
      intégré, reaper de reprise 30 min), dev/prod en `BackgroundQueue`
      `num_workers 1` (contention ~/.platformio) ; `BuildFirmwareWorker` :
      running → run_build → succeeded(+clé)/failed, `Ok(())` sur échec
      (échecs compilation déterministes, pas de rejeu) ; **token + clé
      relus en base au perform** (jamais en queue) ; smoke : worker
      enregistré sous `start --server-and-worker`
- [x] Endpoints parité Django (`controllers/builds.rs`, contrat
      `docs/contracts/build.http`) : POST /build-firmware (ordre Django :
      validation → modèle 400 → device 404 exacte → quota 403 → 429
      intervalle dernier build RÉUSSI vs `min_build_interval_secs` →
      upsert un record par (org, device_id) → 201 `{build_record_created,
      build_id, status, message}` — plus de champs k8s) ; GET
      /build-records paginé D14 + filtres device_id/success ; DELETE
      (400 succès / 400 device existe / 204, artefact conservé D6) ;
      GET /download/firmware/{device_id} (proxy octets, attachment) ;
      phases canoniques queued|running|succeeded|failed (mapping Django
      consigné) — 11 tests de parité (toolchain remplacée par fixtures
      scripts : échec/timeout pilotés par WIFI_SSID, propagation b64
      prouvée bout-en-bout)
- [x] Front : page Builds (formulaire device+WiFi+serveur prérempli,
      liste paginée, badges de phase, téléchargement data-URI, polling
      ~5 s tant que queued/running) ; enregistrement device avec «
      Compiler maintenant » (défaut coché) → build auto après création,
      ligne d'état dans la modale token, erreurs 429/403 en toast ; i18n
      fr/en
- [x] **SSID/mot de passe WiFi en base64** (fix build failed) : un SSID
      contenant un espace (« Chez Shan ») faisait éclater le flag
      `-D WIFI_SSID=\"…\"` de platformio.ini (quote non terminée → échec
      de compilation du firmware). WIFI_SSID/WIFI_PASSWORD passent en
      base64 comme HOST/TOKEN/DEVICE_ID (côté serveur `child_env`, côté
      firmware décodage au setup dans config.h/soil_sensor/4_chan_relay) ;
      fixture/tests pio adaptés (valeurs pilotes b64), Taskfile fw:flash,
      build.sh, docs §2.1
- [x] **`ws_ssl` — schéma WebSocket du firmware configurable** : le firmware
      parlait toujours `wss://` (handshake TLS en échec contre un serveur
      local sans TLS). Champ `CreateBuild.ws_ssl` (bool, défaut `true` =
      parité industrielle) → queue → `BuildSecrets` → env `WS_SSL`
      true/false du sous-process `pio run` ; côté dépôt `pnex-firmwares` :
      define `WS_SSL` (config.h partagé), schéma ws/wss dynamique +
      `setInsecure()` conditionnel (`soil_sensor`, `4_chan_relay`) ;
      front : toggle « SSL WebSocket » dans le wizard et la modale de
      recompilation, défaut selon le protocole de la page (http local →
      ws, https industriel → wss), revue du build affichant
      `ws://`/`wss://{host}` ; docs firmware-build.md §2.1 (6 variables)
      + build.http
- [x] Docs : `build.http` réécrit (contrats Rust + adaptations),
      `firmware-build.md` statut implémenté + écarts, inventory (D5
      révisé, lignes FAIT, ws/firmware/builds neutralisé), .env.example,
      Taskfile `dev:backend:worker`
- [x] Limites assumées : secrets WiFi/hôte lisibles dans
      `pg_loco_queue.task_data` par l'admin DB (parité spec k8s Django,
      purge `cargo loco jobs clear-jobs`) ; S3/rétention D6/logs → O2/
      cache proxy/cancellation tokens différés ; e2e réelle (pio + flash
      ESP) à vivre avec l'utilisateur

**Prochaine : revue humaine Phase 6**, puis suite Phase 5 (lecture
télémétrie/metrics live pour le front, `ws/metrics/live` corrigé du bug
de sujets Django).

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
| 2026-08-16 | **Phase 4 — actuator-channels différés** : la config des canaux actionneurs (CRUD backend, DTO, table) n'est PAS implémentée en Phase 4 ; elle sera conçue avec le chantier M2M (D13) dont dépend sa distribution aux devices. Le périmètre Phase 4 = devices sensors + catalogue | Demandé par l'utilisateur 2026-08-16 (« pour le moment ne traite pas les actuator… réflexion à avoir sur le M2M ») |
| 2026-08-16 | **D14 — Pagination + recherche obligatoires sur toutes les listes** : écart assumé avec le scaffold Django (tableaux nus) ; enveloppe unique `{count, next, previous, results}` (forme LimitOffset DRF) partout, `limit`/`offset`/`search`, défaut `PAGINATION_DEFAULT_LIMIT` (10), max 100 | Demandé par l'utilisateur 2026-08-16 (« on ne garde pas la parité Django, on l'améliore » — sans pagination bornée, base et réponses souffriraient à l'échelle). Détail complet dans `docs/inventory.md` D14 |
| 2026-08-16 | **D16 — Normalisation des noms de mesures** : canonisation (trim, accents pliés, minuscules, séparateurs fondus) avant validation stricte/découverte/stockage ; `Soil-Moisture` ≡ `soil_moisture` ; résultat vide → `error:invalid_format`. Mapping par capacité écarté (trop lourd pour le bénéfice) | Demandé par l'utilisateur 2026-08-16 (« normaliser, c'est plus simple ? ») — choix de l'option légère parmi les 3 proposées |
| 2026-08-16 | **D5 révisé — `ArtifactStore` à deux backends, local d'abord** : stockage d'artefacts abstrait (`local` FS pratique pour l'edge / `s3` pour le cloud), sélection `STORAGE_BACKEND` ; Phase 6 implémente `local`, S3 = plomberie différée | Demandé par l'utilisateur 2026-08-16 (« possibilité d'utiliser un stockage local (pratique pour du edge) ou s3 (cloud) — commencer par du local, STORAGE_BACKEND=local ») |
| 2026-08-16 | **Phase 6 — suivi des builds par polling, WS différé** : le front rafraîchit la liste des records ~5 s tant qu'un build est queued/running ; `ws/firmware/builds` (parité Django) sera réexaminé avec le chantier M2M | Demandé par l'utilisateur 2026-08-16 (choix « Polling d'abord » parmi les options proposées) |
| 2026-08-16 | **D15 — Ingestion : bail anti-clone first-live-wins + sortie metrics OpenObserve** : le premier device qui ingère occupe la place (4003 pour un clone), déconnexion propre libère le bail, reaper désactive après TTL silence (10 s, configurable) ; télémétrie ingérée en Prometheus remote-write (metrics O2, pas les logs), org O2 + token d'ingestion provisionnés automatiquement et correlés en base | Demandé par l'utilisateur 2026-08-16 (« le premier occupe la place → active… si plus de données depuis 10 s → deactivated » ; « les données doivent arriver dans les metrics »). Détail dans `docs/inventory.md` D15, contrat `docs/contracts/ws-sensor-ingest.md` |

## Principes directeurs (confirmés par l'utilisateur)

- **Périmètre pnex-rust = partage de config + capture + ETL.** Rien d'autre.
  Pas d'anti-pattern non industrialisable ou peu robuste (ex : pod K8s par
  actuateur, fan-out Celery par user, contournements de bugs Argo).

## Journal

- 2026-08-19 : **Visualisation — courbes capteur par capteur + formalisation
  du query O2** (demande user : « une petite courbe des données qu'il a
  dans prom/openobserve, un truc à la influxdb, quick and dirty »).
  Constats e2e complémentaires sur O2 v0.92.1 : `query_range` accepte le
  nom nu ET l'égalité `device_id="…"`, rend les points réels sans remplir
  les trous (`values: [[ts,"val"],…]`) ; l'instant
  `last_over_time(nom[24h])` énumère une série par device (catalogue).
  Deux endpoints lecture seule (`/api/v1/telemetry/catalog` et
  `/series?metric&device_id&window=1h|6h|24h`, viewer inclus) :
  `services/visualization.rs` (≠ `services::telemetry` qui est le côté
  INGEST — le module read-side a failli l'écraser, renommé à temps),
  step = fenêtre/120, cap 240 points, passcode→root, timeout 5 s,
  dégradation `available:false` (doctrine dashboard).
  **Anti-injection PromQL** : charset fermé métrique/device + fenêtre
  preset → 400 AVANT toute construction de requête, y compris sans O2
  configuré (bug relevé par le test : le controller court-circuitait
  avant la validation — le short-circuit vit désormais dans le service
  via `Option<&Client>`). Front : page Visualisation (pickers
  métrique/capteur depuis le catalogue, fenêtres 1h/6h/24h, ≤ 6 séries
  superposées avec chips, **chart SVG maison sans lib** — polyline +
  points, échelle Y globale, légende, polling 15 s). e2e réel vérifié
  sur le serveur dev (routes 401 sans token, dégradé 200, injection 400)
  via user Keycloak temporaire créé puis supprimé. **Retour user
  « rien ne s'affiche sur les graphes »** : les logs ne montraient
  AUCUN appel `/series` — piège des selects contrôlés Dioxus (le select
  capteur AFFICHAIT sa première option sans que le signal la porte →
  bouton Ajouter désactivé) ; correction = ancrage par défaut métrique +
  device sur le premier élément valide, et lecture des signaux de la
  ressource `series` dans la partie synchrone de la closure (pattern
  devices.rs, abonnement garanti).

- 2026-08-19 : **Dashboard basic — summary org + page front** (demande user,
  principe « l'UI est la seule interface »). `GET /api/v1/dashboard/summary`
  (une requête pour toute la page, viewer inclus) : liveness PG au TTL de
  silence (≠ booléen `active` du reaper), stats builds (0.0 si vide, jamais
  NaN), dernières mesures **OpenObserve**. **Deux constats e2e sur O2
  v0.92.1** : (1) Basic email:passcode refusé sur la lecture (401,
  ingestion only) → `prom_query` tente passcode puis bascule Basic root
  (le vrai chemin ; mock aligné) ; (2) les sélecteurs `{__name__=~"..."}`
  renvoient un vector **vide**, même avec noms explicites — seuls le nom
  nu ou l'égalité sélectionnent → les noms se découvrent via
  `/api/{org}/streams?type=metrics` puis **une query par métrique**
  (`last_over_time(nom[1h])`), le tout sous timeout 3 s global (retour
  user « last measurement : rien affiché » — la query catch-all
  initiale ne matchait rien sur le vrai O2, le mock l'a révélé en le
  reproduisant). Doctrine tenue :
  jamais de provisioning ni de 500 depuis la branche télémétrie
  (`telemetry.available:false`). Front : 2 cartes org-scope, liste
  liveness, table mesures, polling 15 s (rafraîchissement auto affiché),
  dégradation silencieuse, i18n fr/en. **Caps ~10 côté serveur** (retour
  user 2026-08-19 : « only latest ~10 », pas toute l'org) — liste
  liveness tronquée après tri (compteurs complets, compteur
  « affichés / total » côté front), mesures plafonnées à 10. D6 clos par
  la même occasion (aucune rétention d'artefacts —
  re-flash/recompile toujours possibles). Mock O2 enrichi (route query,
  trace des Basic) ; test.yaml : section openobserve conditionnelle sur
  `PNEX_O2_URL` (Tera).
- 2026-08-19 : **Backend S3 réel — Phase C de D5 v2 (tier industriel)**.
  `S3Store` sur opendal 0.57 (`services-s3` direct, pas la feature loco
  `storage-aws-s3` — pas d'adressage path-style exposé, requis pour
  RustFS & co ; même version que loco → un seul opendal-core) :
  put/get/delete/exists → Operator write/read/delete/stat, layer Retry,
  NotFound opendal → `BuildError::NotFound`. Credentials
  `PNEX_S3_ACCESS_KEY`/`PNEX_S3_SECRET_KEY` (secret masqué dans le Debug
  de `FirmwareSettings`) ; région défaut `us-east-1` (ignorée par
  RustFS) ; **path-style = défaut opendal 0.57** (`enable_path_style` a
  disparu — `PNEX_S3_PATH_STYLE=false` = opt-in host virtuel AWS).
  Validation à la construction : bucket/endpoint/credentials requis →
  erreur explicite, pas d'opé silencieusement cassées. Stub `S3Store`
  du builder supprimé (l'implé vit côté backend, crate builder sans
  dépendance cloud) + variante `BuildError::NotImplemented` retirée.
  Tests : validation config sans réseau + e2e `#[ignore]` **passée
  contre un vrai RustFS** (cycle put/écrasement/delete idempotent/
  NotFound). Stack dev : service `rustfs` (+ init AWS CLI créant les
  buckets `pnex`/`pnex-test`) dans compose.yaml — tier s3 testable
  localement. **MinIO écarté du projet** (licence devenue inacceptable
  pour nous — décision user) : aucune dépendance ni image MinIO, le
  S3-compatible de référence est RustFS. Docs : firmware-build.md (tier
  s3), inventory D5, .env.example.
- 2026-08-19 : **Artefacts firmware en base + bascule postgres/sqlite
  (D5 v2 — trois tiers de déploiement, décision user)**. Backend `db` par
  défaut (`services/artifact_store.rs` + table `firmware_artifacts` :
  upsert `ON CONFLICT (key)` par device → zéro orphelin, sha256, plafond
  50 Mo) ; `local` (FS) supprimé, `s3` = tier industriel différé. Tier
  **sqlite** hobbyiste : tout (données + artefacts + queue
  `sqlt_loco_queue`) dans un fichier — bascule one-knob
  `DATABASE_URL=sqlite://…?mode=rwc` (le `queue.kind` des yaml suit le
  schéma de l'URI via Tera `starting_with`) ; tier **postgres** scalable :
  pods API stateless. Portabilité : `uuid_pk` conditionnel PG
  (gen_random_uuid) dans la migration sites, `Hooks::truncate` portable
  (sqlite : transaction + `PRAGMA defer_foreign_keys`), reaper liveness en
  SQL bindé (plus de `interval` PG), feature `sqlx-sqlite` workspace +
  migration. **⚠ Pas de migration/réconciliation entre tiers** (décision
  user : on choisit à l'installation, rien en prod = wipe autorisé).
  Tests : suite inchangée sur PG + smoke test e2e sqlite
  (`tests/sqlite_smoke.rs` — boot, build fixture, artefact en table,
  download). Vars d'env retirées : `PNEX_ARTIFACTS_DIR`,
  `PNEX_FIRMWARE_SOURCE_*`/`GIT_*` (mortes), `local_root`.
- 2026-08-18 : **CI rouge au merge — stub flasher.js manquant** : `asset!()`
  exige `assets/flasher.js` à la compilation mais le bundle esptool-js est
  gitignoré (généré par `npm run js:build`, comme tailwind.css par
  `css:build`) — les jobs check/test stubbent désormais les deux, et le job
  front build le vrai bundle JS (`npm run js:build` ajouté au côté de
  `css:build`).
- 2026-08-18 : **Convergence monorepo — firmware aplati dans `firmware/` +
  source embarquée dans le binaire** (branche `phase-6-firmware-builder`).
  L'ex-dépôt `pnex-firmwares` (working tree, incl. `common_libs/crypto`
  non commité là-bas) est copié tel quel dans `firmware/` ; `build.sh`
  (secrets réels : SSID, mdp WiFi, token) supprimé — **token et mdp à
  révoquer** (exposés via l'ex-repo public `iot-firmware`, supprimé par
  l'utilisateur). Table rase côté build : `FirmwareSource`
  (Local/Git/clone) **supprimé** — la source est embarquée à la
  compilation (`include_dir!` via build.rs filtré, ~430 Ko) et extraite
  dans un tmp par job (invariant SaaS : `.pio` n'écrit jamais dans la
  source). Config : plus de sélecteur ni de vars
  `PNEX_FIRMWARE_SOURCE_*`/`GIT_*` ; Taskfile racine re-racé sur
  `firmware/` (include `firmware:`, toolchain épinglée O7 préservée) ;
  workflow CI `firmware` compile soil_sensor + 4_chan_relay à chaque
  changement de `firmware/`.
- 2026-08-17 : **Flash firmware navigateur (Web Serial + esptool-js)** sur la
  branche `phase-6-firmware-builder`. Glue JS unique (`js/flasher.js`,
  esptool-js 0.6.1 épinglé, bundlé esbuild IIFE → `assets/flasher.js`,
  pattern Tailwind : tasks `js:build`/`js:ensure`, gitignoré) exposant
  `window.pnexFlash(bytes, onEvent)` ; pont Rust `flash.rs` (js-sys +
  wasm-bindgen, `Closure` d'événements JSON, stubs natifs) ; `FlashModal`
  (téléchargement des octets à l'ouverture, clic = flow complet — geste
  utilisateur requis par `requestPort()`), boutons « Flasher » sur la ligne
  device et l'écran de succès du wizard. L'artefact servi est l'image mergée
  @0x0 (esp8266 image unique, esp32 bootloader+partitions+app) → un seul
  `writeFlash`, paramètres alignés sur le merge serveur (dio/40m/4MB).
  Constat d'exploration : le « esptool.js patché » de l'ancienne UI
  pnex-ui n'a jamais existé (aucun flash navigateur dans le POC React).
- 2026-08-17 : **Itération UI Phase 6 — fusion Builds → Devices** (retour
  utilisateur : la page Builds autonome est inutile). Colonne Firmware
  dans la liste des devices (badge de phase + téléchargement + polling
  5 s) alimentée par un champ `latest_build` sur le DTO Device (hydratation
  batchée côté liste, requête unique côté détail) ; enregistrement
  refondu en **wizard modal** (portage du DeviceWizard React : identifiant
  + shuffle, métadonnées, cartes dynamique/traditionnel, WiFi, revue) qui
  build automatiquement les non-custom et suit la progression **dans la
  modale**, et affiche token + script Python publisher interpolé pour les
  custom ; bouton « Recompiler » (modal WiFi, masqué pour les custom) ;
  page Builds supprimée (endpoints conservés, parité contrat). Correctifs
  O1 (ordre user-info + fallback non-viewer) et O3 (jamais de build
  proposé aux custom) résolus — cf. `docs/observations.md`.
- 2026-08-16 : **Session UI réelle après Phase 6** — trois constats
  consignés dans `docs/observations.md` (registre dédié, à traiter plus
  tard sur demande user) : O1 fallback d'org après re-login sur une org
  viewer (`user-info` sans ORDER BY + `org::clear()` au logout — piégeait
  l'UI en « tout vide »), O3 auto-build proposé à tort pour les devices
  custom (échec propre, parité Django), O4 raison d'échec invisible dans
  l'UI. Au passage : `task dev` lance désormais le backend **avec le
  worker** (commit `81c37a2`) — sans lui, les builds restaient `queued`
  (constat O2 résolu).
- 2026-08-16 : **Phase 6 — worker de build firmware implémentée** (branche
  `phase-6-firmware-builder`) : crate `pnex-firmware-builder` (pipeline
  subprocess pio/esptool + `ArtifactStore` local-first, S3 différé —
  `STORAGE_BACKEND`), queue PostgreSQL loco + `BuildFirmwareWorker`,
  endpoints parité Django (build-firmware, build-records D14, download
  proxy — 11 tests avec toolchain fixture), front Builds + enregistrement
  avec build auto (polling ~5 s, WS différé), docs à jour (contrat
  build.http, firmware-build.md, inventory D5 révisé). En attente de
  revue humaine.
- 2026-08-16 : **Phase 5 (tranche collecte) mergée sur `main`** : go
  utilisateur après e2e réelle (données visibles dans les metrics O2,
  clone rejeté 4003, reaper, `Soil-Moisture` → série canonique
  `soil_moisture` — D16 ajoutée au passage sur sa demande). Gates
  repassées au merge. 7 commits. Prochaine : Phase 6 (worker build
  firmware) — la lecture métrics front de la Phase 5 sera re-planifiée
  après.
- 2026-08-16 : **Phase 5 (tranche collecte) implémentée** (branche
  `phase-5-ingestion`) : WS ingestion ChaCha20 + bail anti-clone + reaper
  + sortie metrics OpenObserve (Prometheus remote-write, provisioning
  automatique correlé en base). Décisions user D15 (TTL 10 s, metrics pas
  logs). Constats techniques O2 v0.92.1 (identifier ≠ name, pas de
  dédoublonnage des noms d'org, /healthz, rôle admin seul natif, Bearer
  passcode non supporté) et loco (ServerOnly par défaut → reaper dans
  after_routes) consignés dans les docs de code. En attente de revue.
- 2026-08-16 : **Phase 4 mergée sur `main`** : revue utilisateur — CRUD
  devices puis tranche pagination/recherche D14 validés. Gates repassés
  au merge (check natif+wasm32, 48 tests, clippy -D warnings, build
  front forcé). Trois commits de clôture : pagination D14, doc
  conception firmware Phase 6, journal PROGRESS.
- 2026-08-16 : **Pagination + recherche D14 implémentées** (par-dessus le
  CRUD devices, même branche) : enveloppe DRF sur les 5 listes de l'API,
  `search` multi-champs (SQL ILIKE sur le catalogue, filtre Rust sur les
  ensembles bornés par quotas), Pager front + recherche avec debounce
  (gloo-timers : futures-timer panique sur wasm32). Doc conception du
  worker firmware Phase 6 ajoutée au passage (contraintes vérifiées du
  dépôt pnex-firmwares).
- 2026-08-16 : **Phase 4 implémentée** (branche `phase-4-devices-crud`) :
  devices CRUD scopé org + catalogue global, backend et front, 8 tests de
  parité (41 au total sur le workspace). **Décision utilisateur : les
  actuator-channels ne sont PAS traités** (DTO retirés du core après coup)
  — la config par canal et sa distribution attendent la réflexion M2M
  (D13). En attente de revue humaine avant merge.
- 2026-08-16 : **Phase 3 mergée sur `main`** : gates repassés au merge
  (check natif+wasm32 sans warning, 30 tests, clippy -D warnings), deux
  imports morts en wasm nettoyés, PROGRESS.md clôturé. Go au merge donné
  par l'utilisateur après les correctifs post-e2e. Prochaine : Phase 4
  (devices CRUD).
- 2026-08-16 : **CI rouge détectée au merge** (rouge en fait depuis les
  commits front, les gates locaux masquaient le problème — le CSS existait
  sur la machine de dev) : `asset!("/assets/tailwind.css")` exige le
  fichier à la compilation mais il est gitignoré, et seuls les builds dx le
  généraient. Fix : task `css:ensure` (stub, sans npm) dont dépendent
  check/test/lint + step stub identique dans les jobs CI check/test
  (vérifié : recompilation forcée bin + bin de test avec le stub seul,
  natif et wasm32).
- 2026-08-16 : **Front Phase 3 implémenté** (suite de la branche
  `phase-3-auth-multitenant`) : port de l'UI `pnex-ui` (React) en Dioxus 0.7 —
  Tailwind v4 + i18n Fluent fr/en obligatoire, client HTTP reqwest (refresh
  401 single-flight, erreurs relayées), login PKCE + callback, shell +
  sélecteur d'org + toasts, pages Organisations/Dashboard/Profil sur les
  endpoints Phase 3, Devices/Builds/Catalog en empty-states, écran URL serveur
  « Bitwarden » architecturé pour desktop (non routé), PATCH /api/v1/profile
  ajouté au backend. Fusion des commits 3-5 du plan (éviter le code mort
  transitoire). En attente de revue humaine.
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
