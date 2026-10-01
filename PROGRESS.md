# PROGRESS — Migration PNEX vers Rust (Loco + Dioxus)

> Pilotage : voir `docs/inventory.md` (registre des décisions). Ne jamais commiter rouge (cargo check / dx build / cargo test).

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
      place du `user` de la stack précédente, dates en RFC 3339, tests roundtrip
- [x] Backend `controllers/devices.rs` : CRUD `/api/v1/devices`
      (réactivation implicite 200 vs 400 device actif, quotas tier par
      type — inactifs comptés, création inactive + DeviceToken auto en
      transaction : token urlsafe 32 o + clé ChaCha20 base64, update
      metadata-only 400 exact, DELETE nettoie build_records + token),
      catalogue authentifié `/api/v1/device-capabilities` + `/api/v1/
      predefined-devices` (filtre capabilities multi OU)
- [x] Durcissements documentés (vs le POC initial) : écriture owner/admin,
      catalogue authentifié, 204 sans body (le body sur 204 de l'ancienne
      stack était illisible côté navigateur), filtre `revision` fonctionnel
      (l'ancienne stack filtrait `version=` inexistant → 500)
- [x] 8 tests de parité HTTP (mock JWKS) : cycle création/réactivation/
      refus, filtres, metadata-only, quotas 3/1/0, isolation tenant,
      rôles, suppression, catalogue partagé
- [x] Front : page Devices (filtres type/statut/capacité + recherche,
      enregistrement depuis le catalogue, détail avec token masqué +
      éditeur JSON metadata, suppression confirmée ; écriture masquée
      viewer), dashboard quotas « x / max » réels, i18n fr/en
- [x] **Pagination + recherche sur toutes les listes (D14)** : enveloppe
      unique `{count, next, previous, results}` (forme LimitOffset de la
      stack précédente)
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
      crée/personnalise. Tables legacy `formula_imports`/`conversion_imports`
      (copie par user) **supprimées**
- [x] **Allègement fluides** (directive user) : catalogue de fluides
      supprimé de la base — le service FastAPI externe (CoolProp/RefProp)
      est la source de vérité, ses erreurs sont renvoyées telles quelles au
      client ; tables legacy `fluid_catalogs`/`fluid_property_groups`
      supprimées, remplacées par `fluid_mixtures` (mélanges custom par org,
      org_id NOT NULL, composition JSONB)
- [x] Durations INTERVAL de la stack précédente → colonnes `*_secs` (bigint)
- [x] Entities SeaORM générées (`cargo loco db entities`, stubs models/)
- [x] `/health/ready` branché sur un `SELECT 1` réel (ok/degraded, testé
      avec PG up et PG coupé)
- [x] Seed idempotent `cargo loco task seed` : fixtures YAML de la stack
      précédente réutilisées telles quelles (5 types, 22 caps, 4 MCU + generic à la
      volée, 4 predefined, 6 tiers, 66 conversions, 39 formules ;
      global_id non-UUID → uuid5 DNS comme dans l'ancienne stack)
- [x] Test d'invariants de schéma (`migration/tests/schema_invariants.rs`) :
      scoping org NOT NULL, catalogue ETL nullable, tables de copie et
      catalogue fluides absents, actions ON DELETE
- [x] Quotas Free unifiés sur le tier fixture (3/1/0) — les 5/2/1 codés en
      dur dans les views de la stack précédente étaient des fallbacks divergents
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
      n°3-4 de la stack précédente (rapport Phase 0 §3). CORS inutile par conception (front
      same-origin servi par le backend) — faille n°1 réglée. Refus par
      défaut via extracteur `AuthUser` — faille n°2.
- [x] JIT provisioning transactionnel (`auth/provisioning.rs`) : users +
      user_profiles + org personnelle owner + tier Free, idempotent,
      re-vérification en transaction (concurrence), resync email/nom.
- [x] Extracteur `OrgContext` (`X-Org-Id` + membership) : point d'ancrage
      du scoping multi-tenant (remplace le filtrage par-viewset de la stack précédente).
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
      connected — remplace Redis db2 de la stack précédente) + `openobserve_orgs`
      (correspondance org PNEX ↔ org O2 + token d'ingestion correlé)
- [x] WS `/ws/sensor/ingest` (`controllers/ws_ingest.rs`, contrat
      `docs/contracts/ws-sensor-ingest.md`) : parité SensorIngest du POC initial —
      auth b64 query (+trim du `\n` firmware), frames
      base64(nonce‖ChaCha20-nu), PING/PONG, key=value, validation
      stricte/découverte plafonnée, erreurs chiffrées, close codes
      4001-4008 ; durcissements : cache revalidation 10 s (§7.8 — l'ancienne
      stack requêtait la DB à chaque frame à ~10 fps), last_seen sur toute frame
      valide, déconnexion propre = bail libéré immédiatement
- [x] **Anti-clone (décision user, D15)** : sessions en-process → 4003
      immédiat + fallback `device_states` frais (crash/autre process) ;
      reaper 5 s seul écrivain de `active` (comportement aligné sur la stack précédente),
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
- [x] Endpoints parité avec la stack précédente (`controllers/builds.rs`, contrat
      `docs/contracts/build.http`) : POST /build-firmware (ordre de la stack précédente :
      validation → modèle 400 → device 404 exacte → quota 403 → 429
      intervalle dernier build RÉUSSI vs `min_build_interval_secs` →
      upsert un record par (org, device_id) → 201 `{build_record_created,
      build_id, status, message}` — plus de champs k8s) ; GET
      /build-records paginé D14 + filtres device_id/success ; DELETE
      (400 succès / 400 device existe / 204, artefact conservé D6) ;
      GET /download/firmware/{device_id} (proxy octets, attachment) ;
      phases canoniques queued|running|succeeded|failed (mapping de l'ancienne
      stack consigné) — 11 tests de parité (toolchain remplacée par fixtures
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
      `pg_loco_queue.task_data` par l'admin DB (comme la spec k8s de la
      stack précédente,
      purge `cargo loco jobs clear-jobs`) ; S3/rétention D6/logs → O2/
      cache proxy/cancellation tokens différés ; e2e réelle (pio + flash
      ESP) à vivre avec l'utilisateur

**ETL Phase 6 — Nœuds device → calc → métrique O2 : IMPLÉMENTÉ** (branche
`flow-engine-phase6-nodes-device`, en attente de revue humaine). La
« partie lecture » : n'importe quel device de l'org se choisit **dans le
nœud** (rien d'imposé à la création du flow), lectures multi-devices des
pins, calculs, écriture du résultat dans OpenObserve **comme une métrique
au même titre que les capteurs** (série `etl_*`, device virtuel
`flow_{id}`, `source_type=etl` — apparaît d'elle-même au catalogue
Visualisation). Pipeline `inject → device → calc → metric` :
- pnex-core : évaluateur d'expressions maison (`calc.rs` pratt parser pur
  wasm-safe, la même fonction valide dans l'éditeur et exécute au runtime),
  nommage centralisé (`naming.rs` — `normalize_measurement_name` déplacé du
  backend pour garantir l'égalité ingestion/lecture), structs prompb en
  feature, 3 kinds + configs + validation + projection estampillant
  `pnex_org_id` dans l'artefact (le runtime déduit l'org O2 sans SQL).
- crate `pnex-node-device` : pnex-device (PromQL `last_over_time`, fenêtre
  de fraîcheur 1..=3600 s, clé omise si donnée absente — jamais de zéro
  inventé), pnex-calc, pnex-metric (remote-write) ; mini-client reqwest ;
  auth Basic racine via allowlist env (OPENOBSERVE_URL/ROOT_EMAIL/
  ROOT_PASSWORD) ; **spike exécuté contre l'O2 réel : écriture racine +
  relecture cohérente ✓**.
- Dépendances pin↔flows : `set_mode` in↔out → scan des flows déployés
  lisant ce pin → **dé-déploiement automatique** (reprojection + SIGUSR1)
  + toast `flow_impacts` UI Pins ; éditeur : violations de **staleness**
  client-only (pin en sortie, pin disparu, device introuvable) → nœud ET
  **câble** en rouge.
- Endpoint `GET /devices/{id}/pinout` (instances + fallback overlay —
  pinout complet même pour un device jamais connecté) ; DeviceForm avec
  selects device/pin filtrés en lecture ; calc avec validation live ;
  metric avec preview `etl_*`.
- Gates : check natif+wasm32, clippy -D warnings, tests (pnex-core 50,
  node-device 7, front 31 + parité i18n). Docs : `docs/architecture/
  flow-engine.md` § 0bis.

**ETL Phase 5 — Éditeur de flows Dioxus : IMPLÉMENTÉ** (branche
`flow-engine-phase5-editeur`, en attente de revue humaine). Canevas SVG
pur Dioxus (aucune dépendance npm/Rust ajoutée, précédent chart
`visualisation.rs`) : palette 4 nœuds (inject, pnex_sql — retiré le 2026-10-01, debug, red),
pan/zoom molette vers le curseur, drag snappé sur grille, câblage
port→nœud, suppression câble/nœud confirmée ; inspecteur par kind (JSON
validés localement, pattern `MetadataEditor`) ; validation locale
`pnex_core::validate_graph` (wasm32) avant save — surlignage des nœuds +
bandeau, 400 serveur traité à l'identique ; save = PATCH avec
`expected_version_number` (409 → modal « Recharger / Écraser ») ;
drawer d'historique (charger une ancienne version = édition → prochain
save crée v(n+1) ; « Déployer cette version » = rollback) ; deploy
gated `can_write && !dirty` avec chip runtime pollé (503 verbatim si
moteur off). Gates : check natif+wasm32, clippy -D warnings, tests
(géométrie/réducteurs purs testés). Docs : `docs/architecture/
flow-engine.md` § Phase 5.

- [x] API glue `src/api/flows.rs` (10 endpoints, types `pnex-core`) +
      `ApiError{status, body}` (409 conflit / 400 violations
      distinguables) ; `Serialize` ajouté sur les DTOs requête pnex-core
- [x] Page `/flows` : liste (filtres search/statut, badges, colonne
      versions « vN · déployée vM », Pager D14, création avec graphe de
      départ inject — l'API refuse un graphe vide, suppression confirmée)
- [x] Canevas + gestes : palette 4 kinds, pan/zoom, drag snappé, câblage
      port→nœud, coupe de câble confirmée, Delete key
- [x] Inspecteur par kind ; violations du nœud affichées (messages
      pnex-core tels quels)
- [x] Save/versioning : validate_graph locale → PATCH → 409 modal deux
      branches ; drawer versions (charger / déployer cette version) ;
      deploy + chip runtime
- [x] i18n fr-FR/en-US (parité testée), clés `flows-*`

**Prochaine : revue humaine Phase 6 + éditeur flows**, puis suite
Phase 5 (lecture télémétrie/metrics live pour le front, `ws/metrics/live`
corrigé du bug de sujets hérité de la stack précédente).

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

**Positionnement** (2026-08-15) : le POC initial n'a jamais passé en prod —
**la version Rust devient la version officielle**. Pas de parité cosmétique
(slashs terminaux, etc.) ; seuls les contrats fonctionnels Phase 0
(`docs/contracts/`) doivent être conservés. Conventions centralisées dans
`convention.md`. `/health/ready` reste honnête sans DB jusqu'en Phase 2.

**Prochaine : revue humaine Phase 1**, puis Phase 2 — Modèles & DB
(PostgreSQL, SeaORM, organisations D2, seed fixtures YAML).

## Décisions

| Date | Décision | Pourquoi |
|------|----------|----------|
| 2026-08-15 | Pas de 2e repo Web UI à réintégrer — le front Dioxus sera écrit from scratch | Confirmé par l'utilisateur (le plan initial mentionnait une Web UI séparée : obsolète) |
| 2026-08-15 | Phase 0 lancée sur le repo de la stack précédente `/home/shan/Documents/shan-perso/pnex-server` | — |
| 2026-08-15 | **Fonctions utilisateur en 2 couches** : (1) ETL d'ingestion en **VRL dans les pipelines OpenObserve** (conversions, champs dérivés, routage) ; (2) fonctions complexes utilisateur en **WASM/wasmtime multi-langage** avec host functions — dont une host fn `coolprop()` qui appelle le service FastAPI côté hôte | VRL **ne supporte pas les appels HTTP** (limitation Vector [#22783](https://github.com/vectordotdev/vector/issues/22783)) → CoolProp injoignable depuis VRL ; enrichment tables CSV inadaptées à la thermo (T,P)-dépendante. Confirmé par recherche 2026-08-15, proposé par l'utilisateur, à valider en revue de phase |
| 2026-08-15 | **VRL abandonné — tout l'ETL en Rust/WASM dans le backend** : OpenObserve devient purement stockage + query + dashboards + reports (pas de pipelines VRL). L'ETL tourne dans Loco (l'ingestion passe déjà par le backend : WS ChaCha20 → Loco → OpenObserve). Deux niveaux côté moteur : (a) **évaluateur d'expressions sûr en Rust** (parité safe_eval de la stack précédente : opérateurs/fonctions/constantes whitelistés) pour les formules/conversions existantes ; (b) **WASM/wasmtime multi-langage** pour les fonctions custom utilisateur, host functions dont `coolprop()` → FastAPI | Un seul moteur au lieu de deux (VRL + WASM) ; le backend est déjà dans le chemin de données donc VRL n'apporte rien ; testabilité cargo ; pas de compétences VRL niche à maintenir. Décidé par l'utilisateur 2026-08-15 |
| 2026-08-15 | **Multi-tenant : l'organisation est le tenant** — 1 org OpenObserve par org PNEX, et **une org peut contenir plusieurs users** (membership `user ↔ org` en PG, avec rôle). Tables : `organizations` + `organizations_members`. Le scoping des données (devices, formules, sites…) passe de `user_id` (stack précédente) à `org_id`. Devices ne parlent JAMAIS directement à OpenObserve — ingestion via WS Loco, le backend écrit dans l'org avec un credential service | L'org est l'unité de tenue native d'OpenObserve (streams, rôles, **retention par org** = aligné sur les tiers d'abonnement Free 1 j → Ultimate 2 ans). Plusieurs users par org = besoin réel (équipes). ⚠️ Nouveau concept vs la stack précédente → impacte schéma PG (Phase 2) et API (Phase 3-4). **Validé en revue** |
| 2026-08-15 | **Rapports → OpenObserve Report Server** : rapports PDF = **scheduled reports de dashboards OpenObserve** (rendu PDF via Report Server, SMTP, cron). Supprime matplotlib + WeasyPrint + le job planifié generate_report de la stack précédente + stockage S3 des rapports | `schedule {cron, email_to}` de ReportConfiguration mappe 1:1. Le layout JSON ReportTemplate était du **code mort**. Formula results déjà indexés dans OpenObserve (`source_type: "formula"`) |
| 2026-08-15 | **D13 — Actuateurs sans serveur au milieu (chantier M2M différé)** : les actionneurs **ingèrent leur propre config**, actuateurs ↔ capteurs **communiquent en direct**. Le serveur garde : stockage/édition des configs (API+UI) + capture/ETL. La mécanique de distribution (broadcast desired-state, push WS, protocole) est reportée à un chantier séparé — ne pas sur-concevoir Phases 4-5 | Vision edge complète confirmée par l'utilisateur ; ws/actuator/cast (config + state) marqué « différé » dans l'inventaire |
| 2026-08-15 | **Revue de phase — points tranchés (D4-D12)** : firmware sur **MinIO/S3 conservé** (abstraction `ArtifactStore`, PG écarté — binaires RTOS/OS complets trop lourds pour PG/backups/WAL) ; rétention artifacts = structure maintenant, gestion plus tard ; rapports = conception détaillée repoussée mais exigences verrouillées (**provisioning/cron OpenObserve par API** via service account, génération live en **tâche backend** anti-saturation) ; **ChaCha20 nu à parité** + versionnement protocole pour upgrade AEAD ultérieur ; **état live device → Postgres** (`device_state` upsert + purge TTL) ; **tokens de la stack précédente supprimés** (JWT Keycloak seul, DeviceToken inchangés) ; **abonnement attaché à l'org** ; **timestamps télémétrie** (D12) : fallback dt d'ingestion + provenance, protocole v2 avec timestamp optionnel, SNTP recommandé côté ESP32 | Revue humaine 2026-08-15 — l'utilisateur a validé l'ensemble et délégué les choix restants ; D12 ajouté suite à sa question sur les devices sans NTP |
| 2026-08-15 | **Renommage service** : `og-device-hub` (héritage de la stack précédente) → `pnex-server` — l'ancien nom n'est plus utilisé | Demandé par l'utilisateur 2026-08-15 |
| 2026-08-15 | **Remote GitHub `Pnex/pnex-rs`** (opensource) ajouté — commits poussés au fil de l'eau | Demandé par l'utilisateur 2026-08-15 |
| 2026-08-15 | **Phase 1 technique** : scaffold Loco v1.0 (`--db none --bg async --assets clientside`) puis trim ; dx 0.7.10 n'a pas de flag `--project` (il faut `cd` dans le crate) et sort dans `target/dx/...` (le Taskfile copie vers `crates/pnex-frontend/dist`) ; assets via macro `asset!()` (manganis hash les fichiers), pas via `[web.resource].style` | Constaté à l'implémentation |
| 2026-08-15 | **Rust = version officielle** (le POC initial n'a jamais passé en prod) : divergences cosmétiques non à justifier, contrats fonctionnels Phase 0 conservés ; conventions (noms, chemins, git, API) centralisées dans `convention.md` ; répertoires des crates renommés `crates/pnex-*` pour correspondre aux noms de packages | Demandé par l'utilisateur 2026-08-15 |
| 2026-08-15 | **Phase 2 — modèle « sans copies »** : fonctions/conversions **standard = fournies par l'app** (`org_id` NULL, partagées en lecture) ; fonctions **user = par org** (ligne matérialisée seulement si l'org crée/personnalise). Tables legacy `formula_imports`/`conversion_imports` (copie par user + suivi de mise à jour) supprimées | Demandé par l'utilisateur 2026-08-15 (« éviter de faire des copies à chaque org/utilisateur ») |
| 2026-08-15 | **Phase 2 — fluides hors base** : catalogue de fluides supprimé de la DB — le service FastAPI externe (CoolProp/RefProp) est la source de vérité et ses **messages d'erreur sont renvoyés tels quels au client** ; la base ne garde que les **mélanges custom par org** (`fluid_mixtures`, composition JSONB) ; `fluid_property_groups` (config app) passe en code Rust | Demandé par l'utilisateur 2026-08-15 (« on va gérer tout côté service FastAPI refprop… alléger la base, sauf mélanges custom ») |
| 2026-08-15 | **Phase 2 technique** : Durations INTERVAL de la stack précédente → `*_secs` bigint ; quotas Free unifiés sur 3/1/0 (tier fixture — les 5/2/1 des views de la stack précédente étaient des fallbacks divergents) ; `global_id` non-UUID → uuid5 DNS (parité bootstrap_db de la stack précédente) ; refs `?` nullable de loco-rs : le « bug » était un mauvais usage (`?` sur le 1er élément, pas de colonne dans `cols`) — corrigé, FK SET NULL via refs pures | Constaté à l'implémentation |
| 2026-08-16 | **Phase 4 — actuator-channels différés** : la config des canaux actionneurs (CRUD backend, DTO, table) n'est PAS implémentée en Phase 4 ; elle sera conçue avec le chantier M2M (D13) dont dépend sa distribution aux devices. Le périmètre Phase 4 = devices sensors + catalogue | Demandé par l'utilisateur 2026-08-16 (« pour le moment ne traite pas les actuator… réflexion à avoir sur le M2M ») |
| 2026-08-16 | **D14 — Pagination + recherche obligatoires sur toutes les listes** : écart assumé avec le scaffold de la stack précédente (tableaux nus) ; enveloppe unique `{count, next, previous, results}` (forme LimitOffset de la stack précédente) partout, `limit`/`offset`/`search`, défaut `PAGINATION_DEFAULT_LIMIT` (10), max 100 | Demandé par l'utilisateur 2026-08-16 (« on ne garde pas la parité avec la stack précédente, on l'améliore » — sans pagination bornée, base et réponses souffriraient à l'échelle). Détail complet dans `docs/inventory.md` D14 |
| 2026-08-16 | **D16 — Normalisation des noms de mesures** : canonisation (trim, accents pliés, minuscules, séparateurs fondus) avant validation stricte/découverte/stockage ; `Soil-Moisture` ≡ `soil_moisture` ; résultat vide → `error:invalid_format`. Mapping par capacité écarté (trop lourd pour le bénéfice) | Demandé par l'utilisateur 2026-08-16 (« normaliser, c'est plus simple ? ») — choix de l'option légère parmi les 3 proposées |
| 2026-08-16 | **D5 révisé — `ArtifactStore` à deux backends, local d'abord** : stockage d'artefacts abstrait (`local` FS pratique pour l'edge / `s3` pour le cloud), sélection `STORAGE_BACKEND` ; Phase 6 implémente `local`, S3 = plomberie différée | Demandé par l'utilisateur 2026-08-16 (« possibilité d'utiliser un stockage local (pratique pour du edge) ou s3 (cloud) — commencer par du local, STORAGE_BACKEND=local ») |
| 2026-08-16 | **Phase 6 — suivi des builds par polling, WS différé** : le front rafraîchit la liste des records ~5 s tant qu'un build est queued/running ; `ws/firmware/builds` (parité avec la stack précédente) sera réexaminé avec le chantier M2M | Demandé par l'utilisateur 2026-08-16 (choix « Polling d'abord » parmi les options proposées) |
| 2026-08-16 | **D15 — Ingestion : bail anti-clone first-live-wins + sortie metrics OpenObserve** : le premier device qui ingère occupe la place (4003 pour un clone), déconnexion propre libère le bail, reaper désactive après TTL silence (10 s, configurable) ; télémétrie ingérée en Prometheus remote-write (metrics O2, pas les logs), org O2 + token d'ingestion provisionnés automatiquement et correlés en base | Demandé par l'utilisateur 2026-08-16 (« le premier occupe la place → active… si plus de données depuis 10 s → deactivated » ; « les données doivent arriver dans les metrics »). Détail dans `docs/inventory.md` D15, contrat `docs/contracts/ws-sensor-ingest.md` |

## Principes directeurs (confirmés par l'utilisateur)

- **Périmètre pnex-rust = partage de config + capture + ETL.** Rien d'autre.
  Pas d'anti-pattern non industrialisable ou peu robuste (ex : pod K8s par
  actuateur, fan-out par user de la stack précédente, contournements de bugs Argo).

## Journal
- 2026-09-12 : **Attachments POI génériques et uniformes** (branche
  `phase-5-attachments-poi-generic`). Registre D42 : `map_pin placed_on`
  admet désormais `tour` (1 ligne, `services/resources/registry.rs`) —
  cibles V1 = media_asset (tous kinds) / dashboard / tour ; les
  viz_diagrams restent retirés. **Hydratation batch** des liens du POI :
  `LinkDto.target_label` (nom résolu org, 1 requête par kind cible via
  `svc::link_target_labels`) + `target_dead` (cible supprimée hors cascade
  → ligne désactivée, jamais 500 — D26) ; supprime le N+1
  `media::detail` par ligne du front. **UI drawer unifiée** (map.rs) : une
  seule section « Objets attachés » — device (colonne D36, 1ʳᵉ ligne,
  détacher = PATCH null) puis 1 ligne par arête (icône par kind, nom
  résolu serveur, Ouvrir/Détacher ; cible morte = ligne grise
  « (supprimé) », Détacher conservé) — un seul bouton « ＋ Attacher un
  objet » ouvrant le **picker générique** `components/resource_picker.rs`
  (onglets Média / Parcours 3D / Tableau de bord / Device, recherche +
  Pager count réel ; le picker ne mute rien, le drawer exécute la
  mutation : POST `/api/v1/resources/edges` `placed_on` ou PATCH
  `device_id`). Viewers : `MediaViewerModal` (photo/floorplan en
  `<img>`, pano/splat via `media_viewer::mount`, asset possédé — plus de
  double fetch) et `TourViewerModal` (école aperçu tour_editor,
  `ViewerSource::Auth`, hotspots lecture). Le `<select>` device du
  formulaire de création/édition reste (décision tracée en doc-comment).
  i18n : clés `poi-panorama-*` retirées au profit de `poi-attachment-*`
  / `poi-picker-*`. Clippy : 4 erreurs préexistantes du code D42 corrigées
  au passage (lint drift rustc 1.96 : `is_none_or`, `nonminimal_bool`,
  `allow` documenté sur `create_edge` 8 args, clone inutile inspector).
  Aucune migration, aucun bump CONTRACT, D36 intact.
- 2026-09-11 : **Viz Phase B — studio SCADA : dashboards à main levée +
  bibliothèque de widgets** (branche `worktree-viz-dashboards`, empilée sur
  le tip viz-concept 1b2b028). Demandes user : « système de PID/graphique à
  main levée type SCADA », bibliothèque modifiable à tout moment, jauges
  sur la télémétrie (dernier avec fenêtre configurable) ou mini-charts,
  versionné avec mode edit/live — arbitrages : **monitoring pur** (pas de
  commande device), bibliothèque **côté serveur par org**, bâtir sur la
  « Base dashboard » de viz-bases.md. Amendements PRD : **D40** (canvas
  libre px — la grille initialement prévue est abandonnée avant
  implémentation — + traits/accroches **purement visuels**, « des traits
  et des accroches » : liaison widget→widget au milieu des côtés, coude
  orthogonal, suit les déplacements, zéro sémantique d'exécution) et
  **D41** (bibliothèque serveur `viz_widget_library`, migration **000016**
  additive ; templates mutables sans versioning — D24 le réserve aux
  documents composites ; l'instance **copie** le modèle au drag :
  snapshot, éditer le modèle ne mute jamais les dashboards). Backend :
  `/api/v1/dashboards` (école flows — save = version = live D24 via
  `current_version_number`, 409 optimiste visant le **pointeur** (un
  restore le déplace et doit confliter), restore re-positionne école
  media), `/api/v1/viz/widgets` (CRUD mutable, unicité (org, name)),
  `POST /telemetry/series-batch` (D31 : un timer par dashboard,
  dégradation **par item**, spec invalide isolée, budget global 8 s,
  anti-injection PromQL inchangée) ; presets fenêtres 5m/15m/30m/1h/6h/24h
  = `pnex_core::VIZ_WINDOW_PRESETS` source unique front/back. Front :
  page `/dashboards` (liste D14 → live/édition par signaux, école
  flows.rs), `dashboard_editor/` (canvas sans lib : div transformé +
  widgets HTML absolus + traits/accroches SVG ; outils Sélection/Trait ;
  drag/resize/snap 10 px/zoom molette ; undo-redo par snapshots — absent
  du flow editor ; save validé dans le navigateur puis PATCH, 409 →
  modale recharger/écraser), `library.rs` (palette, armer-glisser =
  snapshot, « enregistrer comme modèle »), `charts/` (extraction du chart
  de visualisation.rs à rendu identique + instruments : jauge radiale à
  seuils, tuile, mini-courbe, LED, texte). Pièges dioxus conquis :
  signal-champ `cx.x()` interdit (`cloned()`), zéro `set` de signal au
  rendu (dégradé dérivé), closures rsx ne capturent jamais un String
  (relecture du signal sélection), lignes à 2 closures `move` →
  précalcul `Vec<Element>`, `GlobalSignal.set` → `with_mut`. Gates verts
  (check natif+wasm, lint -D warnings, tests workspace hors exemples —
  l'exemple `audit_poses` du stitcher est cassé sur main même, fix en
  attente non commité dans le checkout principal). 7 tests d'intégration
  (cycle save/restore, 409, violations, isolation org, viewer 403, batch
  par item). Reste : E2E interactif, build Android, cross-nav viz_links
  (suite de la Phase B), coordination 000015 poi_first (l'autre session).
- 2026-09-04 : **ETL Phase 6 — nœuds device → calc → métrique OpenObserve**
  (branche `flow-engine-phase6-nodes-device`, sur la Phase 5). L'utilisateur
  avait demandé la modélisation des devices en lecture : choisir n'importe
  quel device **dans le nœud**, plusieurs devices, des calculs, et un
  résultat enregistré « comme une métrique au même titre que les sensors » ;
  il laissait l'architecture libre (« index dédié… je te laisse décider »).
  Décisions structurantes :
  - **Lecture** = PromQL `last_over_time` sur la même série que l'ingestion
    (source de vérité unique O2 ; LAST_VALUES est mémoire-process Loco,
    inaccessible au runtime) ;
  - **« Index dédié »** = préfixe `etl_` + label `source_type=etl` (pas un
    stream O2 séparé — le catalogue Visualisation est une découverte
    dynamique : la série apparaît d'elle-même comme un capteur) ;
  - **Creds** = auth Basic racine via allowlist env + `pnex_org_id`
    estampillé dans l'artefact (zéro SQL/creds dans le runtime) — validé
    par spike sur l'O2 réel (`examples/o2_spike.rs`) : le passcode d'org ne
    couvre pas les lectures O2 v0.92.1, la racine couvre lecture+écriture ;
  - **Évaluateur maison dans pnex-core** (zéro dep, wasm-safe) plutôt qu'une
    crate externe : la même fonction valide (éditeur) et exécute (runtime) ;
  - **`normalize_measurement_name` déplacé dans pnex-core** : si ingestion
    et lecture normalisaient différemment, le PromQL chercherait une série
    inexistante ;
  - **Dépendances pin↔flows** (demande utilisateur en cours de chantier) :
    un `set_mode` in↔out arrête automatiquement les flows déployés qui
    lisent ce pin (dé-déploiement + reprojection + SIGUSR1, toast
    `flow_impacts`) et l'éditeur marque le nœud **et le câble** en rouge
    (staleness client-only : pin en sortie, pin disparu, device introuvable).
  Gates verts à chaque commit ; failure `tenant_isolation::sso_register`
  préexistante et environnement-dépendante (hors périmètre, vérifiée sur
  main propre en Phase 5).
- 2026-09-04 : **ETL Phase 5 — éditeur de flows drag & drop (Dioxus)**.
  Page `/flows` + `components/flow_editor/` (geometry/state/canvas/
  inspector/versions). Choix : canevas SVG pur sans `view_box` (1 unité =
  1 px CSS — conversion `(client − origine − pan)/zoom`, origine mesurée
  au début de chaque geste via `getBoundingClientRect`), zéro nouvelle
  dépendance npm/Rust ; géométrie et réducteurs purs testés (13 tests) ;
  handlers sans capture de `String` (l'id du nœud ciblé est relu du
  signal sélection — piège FnMut/fncaptures) ; validation locale
  `validate_graph` (wasm32) avant save, 409 → modal deux branches
  (recharger/écraser), 400 violations → bandeau + surlignage des nœuds ;
  « Déployer cette version » = rollback serveur (ne crée pas de version).
  `ApiError` porte désormais `status`/`body` (409/400 distinguables) ;
  `Serialize` ajouté sur les DTOs requête pnex-core (CreateFlow/
  UpdateFlow/DeployFlow — le front construit les requêtes typées) ;
  web-sys + features `Element/DomRect/DomRectReadOnly`. i18n fr/en
  complétées (parité testée). En attente de revue humaine.
- 2026-09-04 : **Branding PNeX — logo UI + thème IdP assorti**. Front : wordmark officiel sur la carte login (h-16), variante claire `logo-light.png` (lettres blanches, X rouge) en sidebar h-10 / callback h-12, navy h-8 en topbar mobile, mark X seul en favicon (`main.rs`) ; clés fluent mortes retirées (`login-welcome`, `app-name`, `app-tagline`). IdP : thème light/dark (navy `#151821`, bleu UI `accent/action`, rouge X `error`, cyan `theme_moon`) + logo/favicon X sur clients `pnex` **et** `rauthy` (fallback register/account/admin), via `deploy/rauthy/branding/theme.json` + `apply-branding.sh` (`task rauthy:branding`, clé API admin `pnex-branding` versionnée dans `bootstrap/api_keys.json` — API admin Rauthy 0.36 : header `API-Key`, ni Bearer ni password grant ; routes **sans** préfixe `/admin`) ; client renommé « PNEX UI » → « PNeX » (login page affiche « Login: PNeX »). **Logout** : la page logout Rauthy (0.36, upstream `main` identique) ne fait **jamais** naviguer vers `post_logout_redirect_uri` — son GET ne sert que la page de confirmation (le commentaire « skip the logout confirmation » du source est faux, check inversé) et après le POST elle rejoint toujours sa landing `/auth/v1/` hardcodée. Design retenu : `/api/v1/oauth2/logout` sert un **formulaire HTML auto-soumis** qui POSTe l'end-session Rauthy en TOP-LEVEL — le 302 final de Rauthy vers `post_logout_redirect_uri` (l'origine de l'app, validée par Rauthy) devient une vraie navigation : le navigateur **atterrit sur 5150** (boot déconnecté → écran de login). Seul chemin qui redirige réellement : le GET logout Rauthy ne sert que sa page de confirmation et sa page SPA POSTe en `fetch` pour finir sur sa landing hardcodée — jamais une navigation. Deux pièges corrigés au passage : l'`id_token` doit être capturé **avant** la purge locale (sans `id_token_hint`, Rauthy affiche sa confirmation yes/no au lieu de l'auto-logout — bug hérité du code d'origine) et l'URI de retour doit rester dans la liste autorisée du client (URI hors liste = POST en erreur = SSO non détruit). Wipe du volume Rauthy + re-bootstrap (api_keys.json lu à la 1re init) ; users recréés avec nouveaux `idp_sub` → reprovisionnement JIT pnex au 1er login (ou `db:reset`).
- 2026-09-03 (soir) : **Migration Keycloak → Rauthy (D19) — IdP full-Rust, DB embarquée hiqlite**. compose.yaml : `rauthy` (ghcr.io/sebadob/rauthy:0.36.2, :8080, volume `pnex-rauthydata`, config `deploy/rauthy/config.toml`, bootstrap `clients.json`/`users.json` lu à la 1re init DB) + `mailcrab` (:1080, SMTP catcher pour l'inscription ouverte). Backend : `KeycloakSettings`→`RauthySettings` (issuer `{base}/auth/v1/` **avec slash final**, endpoints `/auth/v1/oidc/*`), `action=register`→ page Rauthy, `action=reset`→ page compte Rauthy ; **2 surprises Rauthy corrigées** : UA obligatoire sur /token (reqwest `.user_agent("pnex-server")`) et JWKS mixte RSA+OKP (parser ignore les entrées sans `n`/`e`) ; **access tokens lean** (pas de `preferred_username`) → `Claims` assoupli, `display_name` retombe sur l'email ; `users.keycloak_uuid` (UUID) → `users.idp_sub` (varchar) — Rauthy émet des `sub` 24 chars, reset DB assumé (app pas en prod) ; User-Agent requis ; tests : mock `/auth/v1/oidc/certs`, `RAUTHY_URL`, issuer `{base}/auth/v1/` — suite backend verte. Vérifié e2e contre Rauthy réel : password grant (email en username) → 200 user-info (JIT org Free) ; refresh immédiat rejeté **par design** (`nbf = exp_AT − 60` — l'UI rafraîchit sur 401, compatible). Front : profil « géré par Rauthy » (i18n). En attente de revue humaine.

- 2026-09-03 (après-midi) : **Quatre correctifs sur les retours UI du
  generic_esp8266 — et un e2e « reboot-proof » complet**. (1) **Ordre des
  pins** : l'ordre SQL/HashMap est arbitraire et changeait d'un poll à
  l'autre (cartes D0/D6/A0 mélangées à l'écran) — tri naturel des labels
  dans `GET /pins` (A0 < D0 < … < D8, préfixe alpha puis numéro). (2)
  **Booléens en télémétrie** : Prometheus n'a pas de booléens —
  `promwrite::series_of` parsait un f64 et jetait silencieusement TOUS les
  StateReports digitaux (`value: true/false`) ; « aucune donnée brute en
  Visualisation malgré un subscribe 1 s ». Fix à la source
  (`ws_device::handle_state_report`) : bool → 1/0 pour la série O2,
  `LAST_VALUES` garde le booléen pour l'affichage HIGH/LOW de l'UI. (3)
  **`adc_in` vs `analog_in`** : le fil sérialise `AdcIn` → `"adc_in"`
  (serde snake_case du proto) mais le firmware ne comparait que
  `"analog_in"` (convention BASE) — A0 tombait en digital_in sur la carte,
  un subscribe A0 aurait lu du digitalRead(17). Firmware tolérant aux deux,
  doc proto clarifiée. (4) **Cadences de lecture perdues à chaque
  reconnect** : double cause — le `ProvisionAck` ne porte pas les
  `interval_ms` (restauration du desired-state ajoutée : les Subscribe
  persistés sont re-poussés après chaque Announce) et **l'upsert
  d'admission réécrivait la config via un round-trip `ModeOpts` — sans
  champ `interval_ms` — effaçant la cadence à CHAQUE re-announce** (leçon :
  la config jsonb est un bagage, ne pas la re-sérialiser par un type plus
  étroit). Firmware : logs `[CMD] set_mode/write/subscribe` + refus d'ack
  systématiques (réponse à « rien ne s'affiche sur le moniteur quand j'écris
  un pin » : normal avant, les prints n'existaient pas). UI : le select
  « Read every » s'initialise à la cadence persistée (`interval_ms` exposé
  dans le DTO) au lieu de retomber sur « manuel » au refresh. **Preuve
  e2e** : reboot réel de la carte par esptool (RAM vidée) → watchdog réape
  la session zombie → reconnexion → cadence survivante en base → Subscribe
  re-poussé → série `d1` à 1 Hz dans O2 (181 pts/3 min) → `/telemetry/series`
  rend les points. Vérifié aussi : `GET /pins` → `['A0','D0',…,'D8']`.

- 2026-09-03 (matin) : **Fin du chantier e2e generic_esp8266 — le 7ᵉ bug
  était un wrap arithmétique**. Le serveur envoyait bien les PONG (prouvé
  par un client device synthétique Python : même clé, framing ChaCha20
  contrepérouvé), mais la carte fermait sa session juste après son 1ᵉʳ
  ping. Firmware instrumenté (prints de frames sur le serial) puis flash
  direct esptool : la carte recevait le PONG, le déchiffrait (« PONG »,
  4 octets) — et déclarait quand même le timeout. Cause : `now` capturé au
  début de `loop()`, `last_pong_ms` posé à `millis()` PENDANT `poll()` →
  `now - last_pong_ms` non-signé **wrappe** à ~4,29 Md → `>= 15000` vrai.
  Fix : recapturer `now = millis()` après `client.poll()`. Méthode
  retenue pour ce type de deadlock : instrumentation println côté serveur
  (le logger Loco filtre les cibles app), client synthétique pour isoler
  le sens du flux, capture série passive via pyserial (le port DTR/RTS
  reset la carte — ne pas confondre « muet » et « absent »).

- 2026-09-03 : **Moteur de flow ETL « Node-RED full-Rust » — Phase 0 + Phase 1
  implémentées** (worktree `worktree-etl-flow-engine`, en attente de revue
  humaine). EdgeLinkd vendored en submodule épinglé (`d0a5e11`, Apache-2.0,
  jamais patché — `vendor/README.md`). Décision **D18** : mode B renforcé —
  notre binaire `pnex-flow-runtime` lie `edgelink-core` (features `core,js`,
  bug amont `rquickjs` non conditionné documenté) + nœuds PNEX ; runtime
  headless supervisé par Loco (`services/flow_supervisor.rs`, backoff
  exponentiel, env enfant en allowlist), **SIGUSR1 = rechargement à chaud**
  via `Engine::redeploy_flows` (aucune surface HTTP, éditeur Node-RED jamais
  exposé). Modèle typé dans `pnex-core/src/flow.rs` (pur, wasm32) +
  projection flows.json (`pnex_flow_id`/`pnex_version` embarqués) ;
  `flows`/`flow_versions` versionnées append-only (FK circulaire PG-only,
  409 concurrence optimiste — écart assumé) ; API `/api/v1/flows` (CRUD,
  versions, deploy/rollback reprojetant tous les flows déployés, runtime) ;
  premier nœud custom `pnex-sql` (SELECT-only au build, contrat typé en
  frontière, sqlx Postgres, DATABASE_URL par env — **retiré le 2026-10-01**). Acceptance PRD verte :
  (a) inject→debug headless lancé/arrêté par Loco, (b) vraie requête SQL via
  le runtime, (c) save sans reload puis deploy v2, (d) rollback v1, (e) 409,
  (f) rejets typés en frontière. Gates : check natif+wasm32, tests workspace,
  clippy -D warnings ; CI + sous-module non récursif + job `arm-check`
  (aarch64/armv7). Docs : `docs/architecture/flow-engine.md`,
  `docs/contracts/flows.http`. En attente de revue humaine.
- 2026-09-02 (nuit, fin) : **Le device générique connecte enfin — deux
  derniers bugs + une alerte de conception**. (4) Le firmware générique
  n'appelait **jamais `cryptoSetKey(ENCRYPTION_KEY)`** : `cryptoReady()=
  false`, les frames partaient en clair (mode mock de `common_libs/crypto`),
  le serveur ne déchiffrait rien — annonce jamais admise, 0 instances,
  « Device not provisioned yet » malgré un device « Actif ». (5)
  **`ws_device` n'avait pas de handler PING→PONG** (parité ingest
  manquante) : le firmware pingue toutes les 5 s, le serveur ne répondait
  jamais → PONG timeout 15 s → boucle de reconnexion infinie. (6) Le
  `ProvisionAck` (10 pins ≈ 600 o) dépassait la taille max de frame du
  firmware (`MAX_WIRE = 232 o`, dimensionné pour les erreurs d'ingest) —
  porté à 1024/1384 o (~2,6 Ko de RAM statique en plus). Leçon : **grep du
  littéral b64 de la clé dans le .bin** — une clé de la base absente de
  l'artefact a tranché entre cinq hypothèses.

- 2026-09-02 (nuit, suite) : **Première connexion d'un device générique —
  trois bugs en chaîne** (jeune e2e réelle). (1) Le firmware générique
  envoyait le **token en clair** dans l'URL WS alors que `decode_param`
  attend du base64 (contrat ingest) → rejet 4002 avant l'annonce ; les
  macros b64 TOKEN/DEVICE_ID passent désormais telles quelles (parité
  soil_sensor). (2) Le wizard/Rebuild pré-remplissait l'hôte avec l'origine
  du navigateur — `localhost:5150` quand l'UI est ouverte sur la machine
  serveur — la carte se connectait à elle-même ; le champ est vide par
  défaut + avertissement LAN en direct (`devices-host-loopback-hint`).
  (3) **Session zombie** : une mort sans close TCP (reflash/power cycle)
  laisse la tâche de session parkée sur `socket.recv()` à jamais —
  l'entrée `DEVICE_SESSIONS` rejette alors toute reconnexion en 4003
  « Device already connected » (la carte était verrouillée dehors jusqu'au
  redémarrage). Watchdog d'inactivité 45 s dans `session_loop` : aucune
  frame pendant 45 s → fermeture, garde libéré. Leçons : la machine de dev
  a DEUX IP LAN (WiFi .16 + Ethernet USB .185) — les deux joignables ; les
  logs serveur ne vont que sur stdout (à redirection en fichier, plus tard).

- 2026-09-02 (nuit) : **Rectification B0.1 : toujours compiler par device + fix suppression UI**.
  Décision utilisateur : pas d'artefact générique réutilisable — le
  generic_esp8266 est compilé PAR DEVICE (config en env de build pio,
  parité soil_sensor) ; le flux secteur PNEXCFG1 est retiré (endpoint
  config-sector, module builder, formulaire FlashModal, clés i18n).
  Fix suppression device : la tâche `spawn` Dioxus est liée au scope —
  l'`on_back` synchrone démonterait DeviceDetail et ABORT la requête
  (ni toast ni suppression) ; navigation + refresh déplacés APRÈS
  l'await. brick0.md : B0.1 marquée abandonnée, §5 réécrit, DoD §9
  ajustée.
  Leçon supplémentaire (fix e0c1a8f→0ae8916) : **chemins d'outils relatifs
  vs cwd du worker** — le Taskfile expose PIO_CMD relatif à la racine mais
  le serveur tourne dans crates/pnex-backend ; `resolve_program` ne
  résolvait que depuis le cwd → spawn impossible pour TOUS les builds.
  Double correctif : chemins absolus `{{.ROOT_DIR}}` côté Taskfile + ancre
  `REPO_ROOT` (CARGO_MANIFEST_DIR) côté `resolve_program`.
- 2026-09-02 (soir) : **Fix build firmware « toujours failed » + seed 5ᵉ carte**.
  Trois causes empilées : (1) fixture overlay YAML en map au lieu d'une
  séquence → seed plantait avant l'insert du device générique (4 cartes au
  lieu de 5) ; (2) toolchain pio disparue (`~/.platformio/penv` effacé) →
  réinstall `uv tool install platformio esptool` ; (3) **cause racine du
  fail en 0,2 s** : `build.rs` n'émettait `rerun-if-changed` que par fichier
  suivi → un projet *nouveau* (`generic_esp8266/`) n'était jamais
  ré-embarqué dans le binaire → « projet introuvable » pour tout build
  serveur du générique. Watch porté sur la racine `firmware/` (scan
  récursif cargo) ; test manuel `manuel_pio_reel_generic_esp8266` (ignored)
  rejoue le chemin exact du worker. Leçon : l'embarquement compile-time est
  une cache — il faut qu'il suive sa source.
- 2026-09-02 : **Brick 0 — firmware générique ESP8266 + socle capabilities
  implémenté** (4 tranches, gates verts à chaque commit ; e2e carte réelle
  restante). **core** : proto `/ws/device` (source de vérité du contrat,
  miroir firmware C++), chip-caps `caps::validate` (point unique : GPIO6-11
  flash SPI interdits, strapping GPIO15 boot-LOW, pull-up GPIO16, A0 ADC
  canal 17), types overlay board. **backend** : migration
  `device_capability_instances`, admission `Announce` policy `Validated`
  (load_overlay → validate → upsert : les modes SetMode survivent aux
  re-announce), `/ws/device` (framing/crypto/close codes repris de l'ingest,
  registre DEVICE_SESSIONS mpsc = downlink, StateReport → last_values +
  série O2 `d5`-style `source_type=generic_gpio`), REST pins/commands/
  config-sector (caps::validate AVANT push, 409 offline — D17 jamais
  d'attente serveur), secteur PNEXCFG1 dans le crate builder (magic+version
  +CRC32 IEEE+JSON clair, pad 0xFF). **firmware** : `generic_esp8266` compile
  (RAM 44 %) — zéro secret au build, secteur PNEXCFG lu au boot, boucle
  Announce/ProvisionAck/SetMode/Write/Subscribe + Ack, forceAllOff sur toute
  perte, PING 5 s/PONG 15 s, backoff 1 s→60 s. **front** : FlashModal
  multi-entrées (writeFlash esptool-js : firmware @0x0 + secteur @0x200000 ;
  formulaire WiFi/hôte quand needs_config — préremplissage hôte/schéma de la
  page), section Pins du détail device (polling 15 s, selects mode/safe-state
  à valeurs effectives, toggle HIGH/LOW, cadences input, D17 : tout est
  bouton). **Décision produit à valider** : quota Free mixed 0→1 (le
  prototypage générique doit rester accessible en Free — 0 aurait tué Brick 0
  pour les comptes Free). Constats : (a) le re-announce ré-exécutant
  l'admission, un replace naïf aurait réinitialisé les modes à chaque
  reconnexion → upsert ; (b) la revalidation token (cache 0 s en test)
  vidait le snapshot des pins à CHAQUE frame → pins chargés dans
  build_snapshot ; (c) NodeMCU **D5 = GPIO14** (pas GPIO5) — piège des
  labels overlay vs numéros silicium, les tests l'ont mordu deux fois.

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
  via user Keycloak temporaire créé puis supprimé. **Deux retours user,
  deux leçons Dioxus** : (1) « rien ne s'affiche sur les graphes » —
  les logs ne montraient AUCUN appel `/series` : piège des selects
  contrôlés (le select capteur AFFICHE sa première option sans que le
  signal la porte → bouton Ajouter désactivé) ; (2) la première
  correction mutait les signaux de sélection PENDANT le rendu →
  « toutes les pages bloquées » : le `set` Dioxus notifie sans comparer
  les valeurs, muter au rendu tant que le catalogue est vide bouclait à
  l'infini et gelait toute la SPA. Correction finale : sélections
  EFFECTIVES calculées (`eff_metric`/`eff_device`, défaut = premier
  élément valide) et zéro écriture de signal au rendu ; signaux de la
  ressource `series` lus dans la partie synchrone de la closure
  (pattern devices.rs, abonnement garanti).

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
  custom (échec propre, comme la stack précédente), O4 raison d'échec invisible dans
  l'UI. Au passage : `task dev` lance désormais le backend **avec le
  worker** (commit `81c37a2`) — sans lui, les builds restaient `queued`
  (constat O2 résolu).
- 2026-08-16 : **Phase 6 — worker de build firmware implémentée** (branche
  `phase-6-firmware-builder`) : crate `pnex-firmware-builder` (pipeline
  subprocess pio/esptool + `ArtifactStore` local-first, S3 différé —
  `STORAGE_BACKEND`), queue PostgreSQL loco + `BuildFirmwareWorker`,
  endpoints parité avec la stack précédente (build-firmware, build-records D14, download
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
  CRUD devices, même branche) : enveloppe paginée sur les 5 listes de l'API,
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
  seed idempotent (fixtures de la stack précédente), test d'invariants, tâches Taskfile db:*,
  CI avec service PG. En attente de revue humaine.
- 2026-08-15 : **Nettoyage Phase 1 (noms & positionnement)** : répertoires des
  crates renommés `crates/pnex-*` (= noms de packages), toutes les références
  alignées (Cargo.toml, Taskfile, CI, configs Loco, .gitignore, doc) ;
  `convention.md` créé (noms, chemins, git, API, langue) ; repositionnement
  « Rust = version officielle, la stack précédente n'était qu'un POC » —
  slashs terminaux de l'ancienne stack abandonnés.
- 2026-08-15 : **Phase 1 — squelette du workspace implémenté** (branche
  `phase-1-squelette`) : 4 crates, Loco v1.0 minimal (health), Dioxus 0.7 CSR,
  Taskfile, CI, doc features. Chaîne vérifiée vert-de-vert (check natif+wasm,
  tests, build release, serving statique). En attente de revue humaine.
- 2026-08-15 : **Phase 0 terminée et validée**. Livrables : `docs/inventory.md`,
  `docs/contracts/` (exemples .http + règles de parité).
  Décisions D1-D11 consignées. Prochaine étape : Phase 1 (squelette workspace).
- 2026-08-15 : démarrage Phase 0. Workspace encore vide.
  Repo source : `pnex-server` (l'ancienne stack : API REST + WS, NATS, ES, K8s).
- 2026-09-09 : **Contrat d'API, versions, scan LAN et porte de compatibilité
  app ↔ serveur** : nouvelle crate partagée `crates/pnex-api-contract`
  (CONTRACT + ServerInfo + compatible, wasm-safe) ; endpoint public
  `GET /api/v1/meta/version` (controllers/meta.rs, école health.rs) ; porte
  de compatibilité front (state/compat.rs + pages/gate.rs, branchée dans
  main.rs entre SERVER_READY et le routeur) ; scan LAN natif
  (pages/lan_scan.rs — 254 sondes :5150 en parallèle, détection de plage par
  URL courante / JNI NetworkInterface / préfixe éditable, connexion via le
  chemin unique apply_server) ; version affichée sur le login et carte
  « À propos » au profil ; clés i18n fr/en ; Taskfile (check wasm + sources
  timestamp des 3 tâches front). Piège noté : `use dioxus::prelude::*` masque
  la dep directe `jni` — qualifier `::jni::sys` (jni-sys 0.3 v1_x) dans le
  code JNI du front. Gates : check natif+wasm+android verts, clippy -D
  warnings, tests (contrat 3, front 47 dont 5 scan, backend meta 1), curl
  live sur instance éphémère (précédence route vs fallback SPA vérifiée),
  E2E device (Xiaomi/adb) : unreachable ≤ 4 s, porte → login, scan validé
  par l'utilisateur. Deux pièges Android corrigés en route (cf. mémoire) :
  `spawn` dioxus annulé au démontage du scope → `dioxus::dioxus_core::
  spawn_forever` pour apply_server ; `RequestBuilder::timeout` reqwest muet
  sur natif → sonde bornée par course avec `util::sleep`. Spinner sur l'état
  Checking (demande utilisateur).
### 2026-09-09 — Média (D21) : bibliothèque versionnée + previews + capture Android (couche 1)

Migration `m20260909_000012_media` (PK UUID école sites, FK circulaire
`current_version_id` PG-only) ; `services/media.rs` (trait `MediaStore`,
FsStore opendal `services-fs` ajouté aux features, S3Store mince, settings
env `MEDIA_BACKEND`/`PNEX_MEDIA_DIR`/`PNEX_MEDIA_MAX_BYTES`), `services/media_sniff.rs`
(GPano → panorama, extensions → splat) ; contrôleur `/api/v1/media` (12
routes, premier chemin body-bytes du repo : `DefaultBodyLimit` par handler,
413 JSON propre, versioning append-only nettoyable, 409 `last_version`,
purge storage vérifiée en test) ; contrat `docs/contracts/media.http`
(pas de bump CONTRACT) ; page front `/media` (liste D14, upload octet-stream
via `client::request_upload` + `FileData::read_bytes`, drawer versions
(delete/restore/ajout), previews : `<img>` blob URL (web-sys `Blob`/`Url`
ajoutés), pannellum + gsplat.js via `js/viewers.js` IIFE (`window.pnexViewers`)
+ pont `media_viewer.rs` (pattern tron.rs) ; capture Android sans permission
CAMERA (`capture.rs` : getCacheDir/FileProvider/intent/polling `spawn_forever`,
FileProvider+file_paths.xml+gradle ajoutés au patch manifest) ; D21 +
`docs/architecture/media.md` (incl. étude panorama mobile : V1 = import GPano,
stitching = R&D future). 3 cibles check vertes (wasm32/natif/aarch64-android) ;
tests média verts (cycle, isolation org, rôles/pagination). À faire (suite) :
E2E Android sur émulateur, previews natives (serving same-origin), vignettes.

## 2026-09-09 — Take 360 : capture panoramique guidée on-device (v1)

**Feat (media) : bouton Take 360 à côté de « Prendre une photo » — 16
frames guidées par capteur, stitching on-device pure Rust, upload
kind=panorama.**

- `crates/pnex-stitcher` (nouvelle crate workspace, pure Rust sans
  OpenCV) : warp sphérique piloté par la pose (yaw/pitch/roll au
  déclenchement), blend plateau, couverture mesurée dans la bande
  couverte (seuil 0,98 — frame manquante refusée), pôles étalés, trous
  intra-bande comblés, GPano XMP en APP1 (sniff backend → panorama).
  17 tests : géométrie algébriquement verrouillée (euler⇄quat
  round-trip), scène synthétique à projection inverse indépendante
  (reprojection ±5, couture de fermeture, pôles, anneau troué refusé).
- `crates/pnex-frontend/src/capture360/` : `guidance.rs` (16 pas × 22,5°,
  tolérances 2/1/4°, dwell 1 s, `decide()` pur testé), `state.rs`
  (atomics purs, école capture.rs), `sensors.rs` (FFI ndk-sys brut —
  `ndk` 0.9 sans module sensor : ASensorManager/ALooper sur thread
  dédié, quaternion → euler via `pnex_stitcher::geom`), `fov.rs`
  (JNI Camera2 getCameraCharacteristics, repli 65°), `wakelock.rs`
  (FLAG_KEEP_SCREEN_ON), `frames.rs` (getUserMedia + canvas 1000 px +
  base64 chunké 128 KiB via `document::eval`), `pipeline.rs` (stitch
  thread OS + mpsc → upload kind=panorama), `overlay.rs` (preview +
  SVG guidage + pastilles i18n).
- **Permission CAMERA au manifest** (patch script, step
  `patch_camera_permission`) : demande runtime déjà câblée dans wry
  (RustWebChromeClient) ; **pre-check ajouté au flux photo** dans
  capture.rs — une CAMERA déclarée mais non accordée fait lever
  SecurityException sur ACTION_IMAGE_CAPTURE (Android ≥ 7).
- Bouton Take 360 (Android + owner/admin) + polling résultat (1 OK /
  2 stitch / 3 upload) + 17 clés i18n fr/en.
- Backend, viewers.js et pnex-api-contract : **zéro modification**
  (additif pur, pas de bump CONTRACT).
- Reste à valider device : smoke test des signes capteur
  (ROTATION_DIR + matrice caméra arrière — constantes isolées),
  perf stitch (`stitch_ms`, knob `out_width` 2048→4096), qualité
  réelle de couture (dérive magnétomètre).

## 2026-09-10 — Take 360 : alignement rotation par modèle IA (pose-model)

**Le ghosting de la première capture 2 anneaux venait des erreurs de
ROTATION des poses capteur** (magnétomètre intérieur) : le raffinement NCC
ne corrigeait qu'en translation (roll gelé, fenêtre ±4°) — incapable de
les absorber. Nouveau chemin serveur (feature cargo `pose-model`,
server-only : le device reste pure Rust) :

- `pnex-stitcher/src/pose_model.rs` (ort/ONNX) : SuperPoint (gris ≤ 896 px)
  + LightGlue — **keypoints normalisés [0,1]** (mesure A/B : 567 matches
  normalisés vs 44 en pixels bruts) ; 1 extraction par frame, matcher seul
  par paire (formes dynamiques, ~30 s desktop pour 28 frames/86 paires).
- `rotation.rs` : Horn (quaternion, itération de puissance — pas de SVD)
  + RANSAC ; erreur angulaire via `atan2(|a×b|, a·b)` (acos y perd toute
  précision f32 sous ~1° — inliers fantômes).
- `align::solve_rotation_gn` : Gauss-Newton 3 DOF (roll libre), résidus
  géodésiques √inliers×**√**Huber(0,5°) — pondérer par h rend le coût
  constant dans la queue → gradient nul, solve immobile (bug corrigé) ;
  région de confiance (pas ≤ 1°, l'overshoot oscillait) ; prior par axe
  (yaw 0.3 faible, pitch/roll 1.5). Métrique de test **libre de jauge**
  (les paires relatives ne voient pas la rotation globale).
- Worker : modèle chargé par job, repli NCC si paires < n/3
  (`AlignReport::fallback`), `PNEX_STITCH_MODELS_DIR` +
  `PNEX_STITCH_MAX_CONCURRENT` (défaut 2, sémaphore tokio — décision :
  RPi 8 Go, < 10 min/job). Modèles committés dans `deploy/models/`
  (fabio-sim/LightGlue-ONNX v1, 51 Mo).
- Validation capture réelle (28 frames, 2 anneaux) : 21 paires acceptées,
  correction max 9,7°, **ghosting éliminé** ; 33 tests stitcher verts
  (solve synthétique < 0,05° en relatif), e2e stitch_jobs 4/4.

## 2026-09-10 — Take 360 : session device + pivot capture continue

**Premiers panoramas réels capturés et uploadés** (2 assets `panorama`
equirectangular en base, JPEG 1024×512 valides avec GPano XMP). La session
device a invalidé le modèle « guidage précis » au profit de la **capture
continue** :

- **Pivot UX** : gate d'alignement (dwell/tolérances) infaisable à la main
  (capteur magnétique intérieur bruité, ring de dwell en boucle). Remplacé
  par : **« Ancrer ici »** (l'utilisateur définit l'origine en visant son
  point de départ — recalable à tout moment) puis **capture continue**
  (1 frame auto tous les ~20° de rotation, intervalle anti-rafales 1,2 s) ;
  le stitcher assemble par coordonnées enregistrées.
- **Bugs device chassés** (voir filelog `files/take360.log`, lisible via
  `adb run-as`) : eval natif n'await pas les promesses (getUserMedia
  fire-and-forget + sondage sync), IIFE non invoquée dans les evals,
  chunks base64 parsés comme du JSON, ALOOPER ident POLL_CALLBACK au lieu
  de LOOPER_ID_USER (capteur muet), push décodé écrasé par la
  journalisation (NoFrames), décodeur JPEG protégé par timeout 3 s/frame,
  seuil de couverture device 0,85 (anneau réel ~93 %), Arc de frames
  recréé à chaque render (fuite → swap-thrash), rendu **sérial** (pool
  rayon global en deadlock sur device — rayon retiré du stitcher).
- **UX** : page media ferme l'overlay au résultat (boucle interne parfois
  morte) ; ligne debug DOM temporaire (filelog fiable) ; logger logcat
  Android ajouté (android_logger, tag `pnex`).
- Preview native toujours web-only (D21 V1) : sur Android le détail
  affiche l'**equirect plat** en repli (validation sur le téléphone) —
  la vue 360 interactive reste sur l'UI web.
- **Fix aperçu web** (2026-09-10) : le paquet npm `pannellum` ne
  réexporte rien (build « global script » — la librairie est posée en
  effet de bord sur `window.pannellum`) ; l'objet module vide faisait
  tomber le mount sur « s.viewer is not a function » → badge « aperçu
  indisponible » **sur le web aussi**. `js/viewers.js` retombe sur la
  globale `window.pannellum` quand l'objet module n'expose pas `.viewer`
  (garde symétrique sur `gsplat.Renderer`). Validé headless : mount OK,
  rendu du pano réel (capture device) dans chromium headless.
- **Fix build wasm** (2026-09-10) : `capture360/filelog.rs` (JNI +
  ndk_context) était compilé sur wasm32 via l'impl natif de
  `media_viewer.rs` → module gated `#[cfg(not(target_arch = "wasm32"))]`.

## 2026-09-11 — Bases de visualisation : PRD (carte, dashboards, parcours 3D, diagrammes)

- **PRD** : `docs/architecture/viz-bases.md` (décisions **D22–D34**) + contrat
  `docs/contracts/viz.http`. Trois « bases » interconnectées — **base map**
  (carte géo tuiles **et** plans d'étages), **base dashboard** (widgets
  `(metric, device_id, window)`), **base parcours 3D** (V1 maquette extrudée,
  V2 panoramas 360 greffés) — plus un **éditeur de diagrammes libre type
  Excalidraw** ; navigation croisée typée partout (repère → dashboard,
  widget → map, nœud 3D → dashboard/métrique). Validations produit préalables :
  parcours **hybride par phases**, base map **les deux**, éditeur **libre
  type Excalidraw**, **tout le PRD d'abord** (implémentation en phases A–E
  après relecture).
- **Capitalisation sur le dormance** : le domaine `sites` (migration 000004,
  sans contrôleur ni front) est activé tel quel (D22) ; les 4 autres tables
  dormantes sont supplantées (`map_pins`, `viz_saved_views`, plans =
  `media_assets` kind `floorplan` **jamais** `svg_files`) mais conservées —
  migration `m20260911_000014_viz.rs` purement additive (D23/D33) ; sort
  final des tables dormantes = « Reste ouvert » (audit `count(*)` avant tout
  drop, une proposition de drop est à l'étude).
- **Choix front actés** : éditeurs = squelette `flow_editor` recopié + undo
  client + `canvas_math.rs` unifié (corrige le gap natif `canvas_rect()`) ;
  viewers JS = Leaflet raster (D27, sans WebGL — Mali) et three.js rendu à la
  demande (D28, école gsplat) via les bundles IIFE `window.pnex*` ; île React
  Excalidraw **écartée** (D29 — sérialiseur Rust `.excalidraw` à la place) ;
  charts = extraction du SVG maison (D30) ; modes live/édition avec polling
  suspendu en édition (D34) ; nav croisée = GlobalSignals + routes à query
  params, liens résolus embarqués dans l'overview (D32).
- **Additifs API** : 5 contrôleurs (`sites`, `dashboards`, `tours`,
  `diagrams`, `viz/links`) + `POST /telemetry/series-batch` (D31) — zéro bump
  `CONTRACT` ; ⚠ proximité documentée `/api/v1/dashboard` (résumé, singulier)
  vs `/api/v1/dashboards` (CRUD, pluriel).
- **Non-buts** : indoor positioning, co-édition temps réel, tout chemin de
  contrôle serveur (D13/D17) ; widget psychrométrique/Mollier relégué en
  extension future (enums de types ouvertes — sans migration).
- **Ajustements utilisateur en cours d'implémentation (2026-09-11)** :
  (1) **D27 basculé Leaflet → maplibre-gl 4.x** — le style de tuiles fourni
  `map.alpine-box.com/style/light-en` est un style MapLibre vectoriel
  Protomaps auto-hébergé (raster Leaflet ne le consomme pas) ; 4.x épinglé
  pour WebGL1 (leçon Mali) ; (2) **`map_pins` = POI générique** — « on doit
  pouvoir définir n'importe quel POI » : `device_id` facultatif assumé,
  pictogramme = **emoji** (palette maison desktop + clavier natif mobile,
  zéro asset image — correction après une première piste « logo uploadé »).

## 2026-09-11 — Phase A viz : migration 000014 + domaine sites (backend)

- **Implémenté** (branche `worktree-viz-concept`) : migration
  `m20260911_000014_viz.rs` (11 tables : buildings, floors, map_pins,
  dashboards(+_versions), tours(+_versions), viz_diagrams(+_versions),
  viz_links, viz_saved_views — additive, tables dormantes 000004
  intouchées, CHECK geo/plan PG-only école FK circulaire) ; entités
  régénérées (`cargo loco db entities`, DB dédiée `pnex_test_viz` —
  la DB dev partagée n'a PAS été migrée) ; `services/sites.rs` (point
  d'écriture unique : org imposée, XOR plan D25, CHECK geo/plan répliqué
  app-level, cascade app-level des `viz_links` sources) ;
  `controllers/sites.rs` (`/api/v1/sites` + buildings/floors/pins +
  `GET /{id}/overview` — un lot borné de requêtes, liens résolus embarqués,
  D32) ; kind `floorplan` ajouté à la liste `KINDS` média (D25).
- **Tests** : `tests/sites.rs` 3/3 (cycle complet avec cascades, XOR +
  emoji, isolation org + viewer 403) ; suite backend verte
  (ai/auth/builds/dashboard/devices/flows/media/meta/openobserve/profile/
  sites/stitch_jobs/tenant_isolation/visualization/ws_ingest) ;
  `regulator` 7/7 KO = baseline préexistante connue (runtime de flow
  indisponible en test — mémoire, aucun lien avec la viz).
- **Pièges du passage** : submodule `vendor/edgelinkd` à ré-extraire dans
  le worktree (`git submodule update --init vendor/edgelinkd` depuis la
  racine) ; tests vacuels si `with_app(...)` n'est pas `.await`é (0.00 s =
  alerte) ; `format::json` renvoie `Result<Response>` (le tuple
  `(StatusCode, format::json(x)).into_response()` marche car
  `Result: IntoResponse`) ; `rust_decimal` non-dép directe → ré-export
  `sea_orm::entity::prelude::Decimal` + `TryFrom` f64.

## 2026-09-11 — Phase A viz : viewer /map (frontend, D27 maplibre-gl)

- **Bascule D27 actée par le style de tuiles de l'utilisateur** :
  `map.alpine-box.com/style/light-en` est un style **MapLibre vectoriel
  Protomaps auto-hébergé** — Leaflet raster ne le consomme pas →
  **maplibre-gl 4.x** épinglé (WebGL1, leçon Mali) ; CORS réfléchi ✓,
  glyphes sans suffixe `.pbf` ✓ (vérifiés au curl avant d'écrire le code).
- **Bundle** : `js/map.js` (IIFE → `window.pnexMap`, école viewers.js) ;
  POI = **markers HTML** — le texte maplibre est du SDF monochrome, un emoji
  couleur n'y passe pas ; un élément DOM l'affiche et reçoit le clic.
  Pont Rust `map_viewer.rs` : mount/unmount/error + clic POI lu par
  **polling** de `window.__pnexMapLastClick` (chaîne JSON, marche wasm et
  natif via document::eval — pas de closure traversante).
- **Page `/map`** (nav « Carte ») : liste sites (D14 + search), création,
  détail = carte géo + bâtiments/étages (chips), plan d'étage = image média
  (blob URL) avec POI plan posés en % (x/plan_width), attache d'un plan
  depuis la bibliothèque média (nouveau filtre kind `floorplan`,
  `MediaKind::Floorplan` ajouté aux matches existants), POI géo/plan
  (palette emoji 12 + clavier), suppression POI/site. Échec mount →
  badge `viz-map-unavailable` (jamais panic).
- **Gates** : `task check` (natif + wasm32) ✓, `task lint` ✓,
  `task build:frontend` ✓ (map.js dans dist), tests backend ✓.
  `task test` rouge **hors périmètre** : `pnex-stitcher` exemple
  `audit_poses.rs` (champ `warp` manquant — cassé à HEAD par eeee679 ;
  validé par `cargo test --workspace --exclude pnex-stitcher` + lib).
- **Idiomes dioxus 0.7 réaffirmés** : cloner `resource.read()` en valeur
  possédée AVANT le rsx (les closures `move` exigent 'static) ; itérer des
  Vec possédés quand l'élément porte une closure ; `key:` veut une chaîne
  formatée ; pas de `rsx!{}` imbriqué dans les branches `else` des enfants ;
  `t!` = `use dioxus_i18n::t;`.

## 2026-09-11 — Fix : la carte ne chargeait pas son style (e2e headless)

- **Symptôme** : détail de site OK, canvas maplibre créé, mais **aucune
  requête** vers le serveur de tuiles, pas de badge d'erreur.
- **Cause racine (deux couches)** : (1) le helper wasm `apply` préfixait
  `null` **dans le tableau d'arguments** → `mount(null, host, opts)` →
  `getElementById(null)` → faux silencieux ; (2) une fois corrigé, le pont
  wasm passe les options en **chaîne JSON** mais `map.js` ne faisait jamais
  `JSON.parse` (seul le chemin natif le faisait) → `style: undefined` →
  maplibre crée une carte **sans style** sans erreur (le warning console
  « There is no style added to the map » ne sort qu'à la sonde).
- **Fixes** : `apply(thisArg, args)` correct ; `map.js mount()` accepte
  objet **ou** chaîne ; badge « Carte indisponible » si `mount()` renvoie
  false (plus jamais de silence) ; carte centrée sur le site (zoom 14)
  quand il n'a pas encore de POI.
- **Méthode** : e2e headless Playwright (login Rauthy 2 étapes scripté,
  `uv run --with playwright`) — captures dans /tmp/viz_shots ; le mount
  manuel en console marchait, ce qui a isolé le chemin d'arguments.

- **Remarque utilisateur (à réouvrir en second temps)** : le parcours
  « site → bâtiment → étage » comme conteneur obligatoire **n'est pas la
  vision retenue** — l'essentiel est de pouvoir poser **n'importe quel POI**
  (avec emoji) sans cérémonie de site ; la structuration en sites sera
  rediscutée plus tard (cf. §9.9 de la PRD : POI sans site).

- **2026-09-11 (soir) — Refonte POI-first de la carte (D35–D39, migration
  000015 destructive)** : pivot utilisateur assumé — « on vire tout ça »
  (site → bâtiment → étage) et « l'app n'a jamais été releasée ». Livré :
  carte globale org plein écran (`/map`), sidebar repliable (recherche,
  filtres device/GPS/emoji, « Tout afficher »), ajout de POI au clic carte
  (palette emoji + localisation libre `location_detail`), **clustering
  backend** (grille par zoom, clusters numérotés, jamais 500 points),
  **device attaché unique** par POI (D36), **panoramas 360° attachés**
  (`viz_links`, premier contrôleur d'arêtes — viewer pannellum en modale),
  **couche GPS live** des devices (`device_positions` + tap non bloquant
  sur le sink télémétrie, convention `latitude`/`longitude`/`gps_*`,
  historique = O2 — socle geofencing/tracking). Tests : `tests/pois.rs`
  6/6 + 5 unitaires bucketing ; `task check` vert. Piège découvert : serde
  confond `"champ": null` et champ absent — sémantique absent/null via
  idiom `deserialize_some` (le double-`Option` nu de la Phase A était mort).
  Recette manuelle headless : cf. scripts `/tmp/viz_*.py` (login Rauthy
  2 étapes, swiftshader).

- **Piège maplibre (corrigé en recette E2E)** : le CSS de maplibre-gl
  n'était pas importé — sans `.maplibregl-marker { position: absolute }`,
  les markers HTML s'empilent SOUS le canvas (flux normal, offset = hauteur
  du host) et sont invisibles tout en existant dans le DOM ; l'attribution
  Protomaps/OSM est aussi déstylée. Fix : `import 'maplibre-gl/dist/
  maplibre-gl.css'` dans js/map.js → esbuild émet `assets/map.css`
  (Taskfile js:map + stub js:ensure + <link> et inline natif dans main.rs,
  école viewers.css). Deuxième piège du même tonneau : le pont wasm passe
  les items en chaîne JSON — `setItems` doit parser comme `mount` (le
  forEach d'une string lève, avalé par le pont). Recette : scripts
  `/tmp/viz_poi_e2e.py` + probes — E2E vert (pins, GPS 🛰️, cluster 207
  visible sur Lyon, drawer détail).
## 2026-09-11 — Studio : parcours de visite 3D versionnés (mode panorama, couche 2 de la vision média)
- **Branche** `worktree-studio` (basée main, 4 commits) — éditeur + viewer
  de parcours multi-étages façon 3D Vista : scènes = panoramas 360 de la
  bibliothèque (D21, référencés par UUID **jamais dupliqués**), posés à la
  main sur des **plans d'étage importés** (nouveau kind média `floorplan` —
  extension sans migration), hotspots de navigation (kind `walk|stair`
  **dérivé** des étages des extrémités), publication (viewer in-app + **lien
  public tokenisé sans login**). Décisions **S1–S14** :
  `docs/architecture/studio.md` (espace Sx — la numérotation D est
  consommée par viz-bases.md sur la branche non fusionnée).
- **Backend** : migration `000014` (`tours`/`tour_versions` — FK circulaire
  `published_version_id` école `deployed_version_id`, `share_token` UNIQUE
  32 hex), types partagés `pnex-core::tour` (serde-only wasm32),
  `services/tour.rs` (append-only + concurrence optimiste 409 + validation
  des assets en base : existence org + kind requis **par usage**), contrôleur
  `/api/v1/tours` (CRUD D14, versions, publish/unpublish, share 409
  `not_published`) + `/api/v1/public/tours/{token}` **sans auth** (école
  health.rs ; doc `no-store`, octets `?v=n` cache `immutable`, 404 uniforme
  — pas d'énumération). ⚠ **collision 000014 assumée avec la branche
  worktree-viz-concept** (renumérotation à la convergence, précédent
  ai_connectors 000010) — la base dev partagée contenant déjà les tables
  viz, le studio utilise `pnex_studio` (dev) + `pnex_test_studio` (tests).
- **Front** : page `/studio` ↔ `TourEditor` (squelette flow_editor :
  geometry/state purs testés, ScenePin/Canvas SVG sans viewBox,
  MediaPicker par kind, drawer versions + publication + lien copiable),
  viewer `pnexViewers.tour` (pannellum **unique** partagé avec le média,
  hotspots clickHandlerFunc synchrones, **une seule scène montée** —
  switch = destroy+remount), nav par global chaîne JSON pollée (pas de
  closure traversante — marche web + webview), page `/share/:token`
  **hors layout** (précédent AuthCallback) avec `/api/v1/public/*`
  exempté d'auth dans `api/client.rs` (jamais de Bearer/X-Org-Id ni de
  refresh 401 dessus). i18n registre `studio-*`/`tour-*` (parité fr/en).
- **Fix traversant** : `examples/audit_poses.rs` cassé sur main depuis le
  warp local (champ `PreparedFrame::warp` manquant — le fix existait en
  non-commité sur le checkout principal).
- **Connaissance** : `task test` = 13 binaires verts + tours 6/6 ; seuls
  les **7 régulateurs** restent KO (échecs préexistants documentés,
  « runtime de flow indisponible » — non imputables, baseline HEAD propre).
