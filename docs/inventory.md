# Inventaire de migration PNEX — Django → Rust (Loco + Dioxus)

> Phase 0. Source de vérité : `pnex-server` (Django 6 + DRF + Channels).
> Détails d'exploration : `docs/phase0/*.md`. Pilotage : `migration.md`, `PROGRESS.md`.
> **Tout ce qui est marqué « SUPPRIMÉ » ne doit pas être réintroduit** (migration.md §3).

## 0. Décisions structurantes ajoutées en Phase 0 (à valider en revue)

| # | Décision | Impact |
|---|---|---|
| D1 | **Pas de VRL — tout l'ETL en Rust dans le backend** : (a) évaluateur d'expressions sûr en Rust (parité `safe_eval` Django) pour formules/conversions ; (b) WASM/wasmtime multi-langage pour les fonctions custom utilisateur, host functions dont `coolprop()` → service FastAPI. OpenObserve = stockage + query + dashboards + reports uniquement (pas de pipelines VRL). L'ingestion passe déjà par Loco (WS → backend → OpenObserve), donc aucun besoin d'un moteur de transformation côté OpenObserve | Phase 5/8 |
| D2 | **L'organisation est le tenant** : tables `organizations` + `organizations_members` en PG ; 1 org OpenObserve par org PNEX ; **plusieurs users par org** ; scoping `org_id` au lieu de `user_id`. Nouveau concept vs Django | Phases 2, 3, 4 |
| D3 | **Rapports → OpenObserve** : scheduled reports de dashboards (Report Server + SMTP + cron). Supprime matplotlib/WeasyPrint/Celery/S3-rapports | Phase 5/8 |
| D4 | Le chiffrement device actuel est **ChaCha20 NU (sans Poly1305)** — migration.md dit « ChaCha20-Poly1305 ». Décision à prendre : compatibilité exacte (nu) ou upgrade AEAD (breaking firmware) | Phase 5 |
| D5 | **Firmware → MinIO/S3 conservé** (abstraction `ArtifactStore` en Rust ; disque possible en dev). PG large objects **écarté** : binaires potentiellement lourds (RTOS/OS complet), backups/WAL gonflés | Phase 6 |
| D6 | **Rétention des artifacts : structure posée maintenant, gestion plus tard** — clés `org_{id}/firmware/…`, champ/config de rétention + job worker placeholder | Phase 6 |
| D7 | **Rapports : discussion de conception repoussée** ; exigences verrouillées : (a) tout OpenObserve doit être provisionnable/cron-able **via API** (service account — orgs, dashboards, reports) ; (b) génération à la demande = **tâche backend** (job queue PG) pour éviter la saturation | Phase 8 |
| D8 | **ChaCha20 nu à parité stricte** (compatibilité firmware ESP32 existant), versionnement du protocole de chiffrement pour permettre un upgrade AEAD ultérieur | Phase 5 |
| D9 | **État live device → PostgreSQL** (table `device_state` : upsert + purge périodique type TTL logique) — cohérent « que du PG » ; Redis db 2 supprimé | Phase 5 |
| D10 | **Tokens DRF supprimés** — auth user = JWT Keycloak uniquement ; devices = DeviceToken (inchangés) | Phase 3 |
| D11 | **Abonnement/tier attaché à l'org** (pas au user) — cohérent avec D2 : plusieurs users par org, un abonnement par org | Phase 2/3 |
| D12 | **Timestamps télémétrie** : (a) le backend accepte un timestamp device **optionnel** ; s'il est absent → **dt d'ingestion serveur** (trace garantie) ; (b) provenance stockée dans le doc (`ts_source: "device"\|"server"`), `_timestamp` OpenObserve = ingestion → deux traces ; (c) v2 du protocole d'ingestion versionné avec timestamp optionnel (comme D8 pour le chiffrement) ; (d) SNTP côté ESP32 recommandé (trivial, connecté internet) — indépendant de la migration, mais nécessaire pour le buffering hors-ligne futur (mesures différées correctement datées) | Phase 5 |

## 1. Apps Django → destins

| App | Contenu | Destin |
|---|---|---|
| authent | Keycloak JWT + proxy OAuth2 + préférences | → Loco Phase 3 |
| devices | Registre devices, tokens, configs actuateurs | → Loco Phase 2/4/5 (config broadcast) |
| dev_ctl | Consumers WS, NATS→ES/Redis, sync statut | → Loco Phase 5 (restructuré) |
| etl | Formules, conversions, fluides, rapports | → Phases 5/8 (OpenObserve + WASM) |
| metrics | Lecture ES (modèles PG hérités) | → Phase 5 (OpenObserve) ; modèles PG SUPPRIMÉS |
| sites | Sites/SVG/annotations | → Loco Phase 4+ (tranche verticale après devices) |
| subscription | Tiers + profils | → Loco Phase 2/3 (+ concept org D2) |
| firmware_builder | Builds firmware | → worker Loco Phase 6 |
| k8s_ctl | StatefulSets actuateurs | **SUPPRIMÉ** (§3 : régulation à l'edge) |
| bootstrap_db | Fixtures YAML | → seed Phase 2 (mêmes YAML réutilisables) |
| health | Probes | → Loco Phase 1 |
| simulator | sim_sensors | → outillage dev Rust (utile pour tests parité) |
| argo_wf | Templates Argo | **SUPPRIMÉ** (§3) |

## 2. Modèles (détail : phase0/models.md)

| Modèle Django | Cible | Phase |
|---|---|---|
| (nouveau) `organizations`, `organizations_members` | SeaORM — **D2** | 2 |
| User Django | users SeaORM (JIT provisioning Keycloak) ; plus orgs | 2/3 |
| SubscriptionTier / UserProfile | SeaORM (retention par org ? — voir §7) | 2/3 |
| DeviceType, DeviceCapability, MCUBoard, PredefinedDevice | SeaORM (catalogue global + fixtures YAML) | 2 |
| DeviceRegistry (+ discovered_measurements) | SeaORM ; scoping **org_id** (D2) | 2/4 |
| DeviceToken (token + encryption_key) | SeaORM ; hook génération (token_urlsafe(32) + clé ChaCha20) | 2/4 |
| ActuatorChannelConfig | SeaORM — **schéma du desired-state broadcasté** ; logique de contrôle (modes, hystérésis) évaluée à l'edge | 2/5 |
| UnitConversion, Formula, FormulaDataSource | SeaORM | 2/8 |
| FluidCatalog, FluidPropertyGroup | SeaORM (validation via CoolProp FastAPI) | 2/8 |
| FormulaImport, ConversionImport | SeaORM | 2/8 |
| ReportTemplate, ReportConfiguration, ReportExecution | **SUPPRIMÉS (D3)** — remplacés par dashboards + scheduled reports OpenObserve | 5/8 |
| Site, SVGFile, SiteDiagram, Annotation, SavedView | SeaORM (PK UUID) | 2/4 |
| metrics.Metrics, metrics.Ping | **SUPPRIMÉS** — héritage PG, lecture déjà ES | — |
| BuildRecord | SeaORM (sans champ argo_wf_job_name) | 2/6 |

Logique save()/clean()/signals → hooks SeaORM + validation service (détail models.md §12).

## 3. API REST (détail : phase0/api-rest.md)

| Groupe endpoints | Cible Loco | Phase |
|---|---|---|
| /health/live, /ready | Loco (idem, cache→OpenObserve check) | 1 |
| /api/v1/oauth2/* (token, refresh, sso, test) | Loco proxy Keycloak (PKCE S256) | 3 |
| /api/v1/user-info, preferences, device-statistics | Loco + concept org | 3/4 |
| /api/v1/devices CRUD (+ réactivation implicite, quota tiers, metadata-only) | Loco | 4 |
| /api/v1/device-capabilities, predefined-devices | Loco ( catalogue global) | 4 |
| /api/v1/actuator-channels CRUD + by_device | Loco (desired-state) | 4/5 |
| /api/v1/actuator-channels/pod-status/{id} | **SUPPRIMÉ** (pods K8s) | — |
| /api/v1/metrics (ES query) | Loco → OpenObserve query API | 5 |
| /api/v1/live-metrics (Redis db 2) | Loco (voir §5 état live) | 5 |
| /api/v1/build-firmware, download, build-records | Loco + worker | 6 |
| /api/v1/sites/* (5 viewsets UUID) | Loco | 4+ |
| /api/v1/etl/unit-conversions, formulas (+evaluate, import), global-*, imports | Loco (+ moteur WASM) | 8 |
| /api/v1/etl/templates, configurations, executions, generate | **SUPPRIMÉS (D3)** — OpenObserve reports | — |
| /api/v1/etl/fluids, devices | Loco | 8 |
| /api-token-auth/ | À trancher (DRF token pour WS auth) — voir §5 | 3 |

Constats de parité importants (api-rest.md §11) : pas de pagination (tableaux nus) ;
204 avec body JSON ; réactivation implicite POST /devices/ (200 vs 201) ;
PUT/PATCH devices = metadata only ; suffixes .json ; 3 schémas d'auth actifs.

## 4. WebSocket / Channels (détail : phase0/ws-channels-crypto.md)

| Élément | Cible | Phase |
|---|---|---|
| ws/sensor/ingest (ChaCha20, key=value, PING/PONG, capabilities, dynamic measurements) | Loco WS Axum — parité comportementale | 5 |
| ws/actuator/cast — **partie CONFIG** (send_initial_config, push Protobuf chiffré) | Loco WS — **desired-state broadcast** (« version N + hash », edge pull le delta) | 5 |
| ws/actuator/cast — **partie STATE** (réception ActuatorState → docs unifiés) | Loco WS — collecte télémétrie | 5 |
| ws/actuator/cast — **flux sensor_data agrégée** (on_nats_sensor_data) | **SUPPRIMÉ** — dépendait des pods de contrôle ; l'edge agrège lui-même | — |
| ws/metrics/live (dashboard) | Loco WS — **corriger le bug de sujets** (`sensors.*` vs `sensor.*.*.measurement.>`) | 5 |
| ws/etl/formulas/evaluate | Loco WS → query OpenObserve + moteur WASM | 8 |
| ws/firmware/builds (notifications) | Loco WS (job queue PG → push) | 6 |
| crypto_utils.py ChaCha20 nu | crate Rust `chacha20` — **D4 : nu ou Poly1305 ?** | 5 |
| actuator_message.proto | réutilisé tel quel (prost-2) | 5 |
| Auth device WS (token+device_id base64 query) | idem Loco | 5 |

## 5. Infra asynchrone & données (détail : phase0/celery-nats-k8s-firmware.md, etl-es-metrics.md)

| Élément Django | Cible | Phase |
|---|---|---|
| NATS (tous topics sensor/actuator/etl) | **SUPPRIMÉ** — ingestion directe WS → OpenObserve (via backend) ; broadcast config en push WS ; interne éventuel LISTEN/NOTIFY | 5 |
| Elasticsearch + indices user_* + mappings time_series | **OpenObserve** — streams par org (D2), dimensions (device_id, metric_name, source_type), retention par org/tier | 5 |
| Consume NATS → ES (batch 500/10 s) | **SUPPRIMÉ** — écriture directe backend → OpenObserve (bulk) | 5 |
| etl_compute (event-driven formulas NATS) | Moteur ETL **dans Loco** : évaluateur Rust + WASM (D1) — déclenchement événementiel interne | 5/8 |
| Redis db 0 (Celery) + beat (5 tâches) | **SUPPRIMÉ** — worker Loco, queue PG (fan-out par user → jobs itératifs) | 6 |
| Redis db 1 (channels layer) | **SUPPRIMÉ** — notifs WS gérées par Loco | 6 |
| Redis db 2 (device state : pings, last values, états actuateurs) | À décider : Postgres (TTL logique) ou garder Redis — **point ouvert** | 5 |
| k8s_ctl + pods compute par actuateur + run_compute_controller | **SUPPRIMÉ** — régulation à l'edge (M2M) | — |
| Argo Workflows + backend argowf | **SUPPRIMÉ** | — |
| firmware build (k8s_job script : git clone → pio run → esptool merge-bin → S3) | crate `firmware-builder` (shell-out toolchain), job Loco, timeout dur, secrets scopés | 6 |
| MinIO/S3 (firmware binaires + rapports) | Rapports : **SUPPRIMÉ (D3)**. Firmware : **MinIO/S3 conservé** (D5) derrière `ArtifactStore` ; rétention différée (D6) | 6 |
| CoolProp in-process (5 points d'injection) | **service FastAPI externe conservé**, appelé par host fn WASM + validation catalogue | 8 |
| Rapports matplotlib/WeasyPrint/Celery | **SUPPRIMÉ (D3)** — OpenObserve Report Server + SMTP + cron | — |

## 6. Auth & tenancy (détail : phase0/auth-tenant-bootstrap.md)

| Élément | Cible | Phase |
|---|---|---|
| KeycloakJWTAuthentication (JWKS RS256, aud account|client_id, JIT user) | Loco — **corriger** : vérif iss, aud stricte, rôles si besoin | 3 |
| Isolation par filtrage user dans chaque viewset | **Renforcer** : guard/middleware global + scoping **org_id** (D2) | 3 |
| CORS_ALLOW_ALL_ORIGINS=True | **Corriger** — origines explicites | 3 |
| Pas de DEFAULT_PERMISSION_CLASSES | **Corriger** — deny par défaut | 3 |
| Quotas Free incohérents (3/1/0 vs 5/2/1 selon le chemin) | **Unifier** | 2/3 |
| Proxy OAuth2 + SSO PKCE + EmailOrUsername backend | Loco | 3 |
| Fixtures bootstrap_db YAML | réutilisées telles quelles (seed Phase 2) | 2 |

## 7. Revue humaine du 2026-08-15 — points tranchés

Les points ouverts de la première passe ont été résolus (décisions D4-D11 en §0) :

1. ~~Chiffrement~~ → **D8** : ChaCha20 nu à parité, versionnement pour upgrade AEAD.
2. ~~Redis db 2~~ → **D9** : Postgres (`device_state`).
3. ~~/api-token-auth~~ → **D10** : supprimé, JWT Keycloak seul pour les users.
4. ~~Frontière évaluateur Rust ↔ WASM~~ → évaluateur Rust pour les expressions
   existantes (parité `safe_eval`), WASM pour les fonctions custom multi-langages ;
   format de distribution des modules (upload, versioning, signature) à définir
   en Phase 8.
5. ~~MinIO~~ → **D5** : conservé pour le firmware derrière `ArtifactStore` ;
   **D6** : rétention structurée mais gestion différée.
6. ~~Retention par org vs tier~~ → **D11** : le tier s'attache à l'org.
7. ~~Rapport ad hoc~~ → **D7** : conception repoussée ; provisioning/cron O2 par
   API exigé, génération live en tâche backend.
8. **Bugs Django à ne pas reproduire** (confirmé par l'utilisateur) — comportements
   corrigés attendus en cible (ils deviennent le contrat de référence des tests
   de parité) :
   - Dashboard live : souscription sur `sensor.*.*.measurement.>` (via le
     WildcardBuilder équivalent), **plus** le pattern pluriel `sensors.*` —
     le flux live doit effectivement recevoir les mesures.
   - `/metrics/` : lire `@timestamp` (et non `timestamp`) → `event_time` rempli.
   - Endpoints morts (`save_view` annotations, `emqx/authn`) : **restent absents**.
   - `download_url` : cohérent avec la vraie route (le concept disparaît de toute
     façon avec D3 — rapports OpenObserve).
   - Cache TTL de validation device (code mort en Django) : décision explicite
     côté Loco — **oui, un cache** (10 s, comme prévu par la config WS), puisque
     la validation DB par message était une lacune, pas une fonctionnalité.

1. **D4 chiffrement** : ChaCha20 nu (compatibilité firmware ESP32 existant) vs
   Poly1305 (robustesse). Recommandation : garder nu à parité stricte Phase 5,
   prévoir versionnement du protocole pour upgrade ultérieur.
2. **Redis db 2 / état live** : Postgres TTL logique vs Redis conservé. Impacte
   /live-metrics, pings (duplicate-connexion 12 s), états actuateurs.
3. **/api-token-auth (DRF tokens)** : les WS dashboard + WS evaluate s'authentifient
   avec. En cible, tout devrait être JWT Keycloak → supprimer les tokens DRF ?
   (les DeviceTokens restent, eux, pour les devices).
4. **Frontière évaluateur Rust ↔ WASM (D1)** : les formules/conversions existantes
   (expressions type `safe_eval`) vont dans l'évaluateur Rust à parité stricte ;
   le WASM est pour les fonctions custom multi-langages futures. Format de
   distribution des modules WASM utilisateur (upload, versioning, signature) à définir.
5. **MinIO** pour firmware uniquement (D3 a supprimé les rapports) : conserver /
   disque / PG large objects. Avant Phase 6.
6. **Retention par org vs par tier** : en D2 plusieurs users par org — le tier
   d'abonnement s'attache à l'org ou au user ?
7. **Rapport ad hoc** (génération à la demande hors cron) : export dashboard
   manuel suffit-il ?
8. Bugs Django à ne PAS reproduire : sujets NATS dashboard incohérents ;
   `timestamp` vs `@timestamp` dans metrics ; endpoints morts (`save_view`,
   `emqx/authn`) ; `download_url` rapports incohérent.

## 8. Proposition de plan de tests de parité (Phase 4+)

- Capturer les contrats depuis `/schema/` (OpenAPI drf-spectacular) du Django vivant
  + exemples requests/*.http (déjà consignés dans phase0/api-rest.md §10).
- Tests d'égalité de comportement : réactivation device, quotas, erreurs 400/401/403/429,
  validation ActuatorChannelConfig par mode, évaluation formules (jeu de référence),
  ingestion WS (codes de fermeture 4001-4008, messages d'erreur chiffrés).
