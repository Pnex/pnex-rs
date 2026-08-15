# Inventaire — Auth / Multi-tenant / Bootstrap / Health (pnex-server)

> Rapport d'exploration Phase 0. Cible : Phase 3 Loco + Keycloak, isolation tenant stricte.

## 1. Authent

### KeycloakJWTAuthentication (authent/authentication.py:16-226)
- Config : `KEYCLOAK_URL` (déf. https://kc.pnex.io), `KEYCLOAK_REALM` (déf. pnex-realm),
  `KEYCLOAK_CLIENT_ID` (déf. pnex). OIDC discovery → jwks_uri (cache 1 h).
- Déclenché si `Authorization: Bearer ...` ; sinon rend la main (chaîne vers
  TokenAuthentication/Session).
- **Validation = JWKS locale, PAS d'introspection** : `PyJWKClient(cache_keys=True)`,
  `jwt.decode(algorithms=["RS256"], verify_signature=True, verify_exp=True,
  verify_aud=False)` ; **audience validée manuellement** : `aud` ∈ {"account",
  client_id} (string ou liste). ⚠️ Pas de vérification d'`iss` explicite.
- **JIT-provisioning** `_get_or_create_user()` (:150-214) : claims
  `preferred_username` (obligatoire), `email`, `given_name`, `family_name` ;
  get_or_create username + sync email/noms ; filet de sécurité création UserProfile
  Free (doublon du signal subscription). ⚠️ **Aucun mapping de rôles Keycloak**
  (realm_access/resource_access jamais lus ; is_staff/is_superuser jamais posés par JWT).
- `authenticate_header()` : `Bearer realm="api"`.

### Endpoints OAuth2 (authent/oauth2_views.py)
| Endpoint | Permission | Flux |
|---|---|---|
| `POST /api/v1/oauth2/token/` | AllowAny | Proxy Keycloak token endpoint. 2 grants : **password** (username/password) et **authorization_code+PKCE** (code/code_verifier/redirect_uri). client_secret ajouté si présent. |
| `POST /api/v1/oauth2/refresh/` | AllowAny | Proxy grant_type=refresh_token. |
| `GET /api/v1/oauth2/sso/` | AllowAny | **302** vers authorize Keycloak, `kc_action=register|UPDATE_PASSWORD`, scope openid profile email, **PKCE obligatoire** (S256), redirect_uri = param ou `{origin}/auth/callback`. |
| `POST /api/v1/oauth2/test/` | IsAuthenticated | Infos user authentifié. |

Autres : user-info/, user/preferences/(update/), user/device-statistics/ — voir api-rest.md §3.

### EmailOrUsernameBackend (authent/backends.py:5-37)
`"@" in username` → lookup email sinon username ; hasher appelé sur user inexistant
(atténuation timing). Utilisé pour admin/session + `/api-token-auth/`. BCrypt seul hasher.

### Commandes management
- `create_su` : superuser depuis env DJANGO_SUPERUSER_*, tier Admin.
- `create_oauth_user` : crée/update user Django depuis Keycloak (password grant →
  validation JWKS → fallback création directe), tier Admin, password=None
  (« Password managed by Keycloak »).

## 2. settings.py — AUTH

- `AUTHENTICATION_BACKENDS` : EmailOrUsernameBackend, ModelBackend.
- DRF : `DEFAULT_AUTHENTICATION_CLASSES` = KeycloakJWT, TokenAuthentication,
  SessionAuthentication (ordre). Throttling anon 5/min, user 1000/min.
  ⚠️ **Pas de DEFAULT_PERMISSION_CLASSES** → AllowAny par défaut sur les vues
  qui ne déclarent pas permission_classes.
- Middleware : HealthCheckMiddleware (1er), corsheaders, CommonMiddleware (**×2,
  dupliqué** :91/:94), Security, Session, Csrf, Authentication, Messages,
  XFrameOptions, WhiteNoise.
- **CORS_ALLOW_ALL_ORIGINS = True** ⚠️.
- Hosts : DJANGO_ALLOWED_HOSTS (+ `health-check-internal` pour probes).
- DB : SQLite si DEBUG sinon PostgreSQL.
- User Django standard (pas de model custom).

## 3. Multi-tenant — mécanisme

**Isolation 100% applicative** : filtrage `user = request.user` dans chaque
`get_queryset`/`perform_create` + FK `user` sur chaque modèle propriétaire +
index ES `user_{id}_measurements` + dossiers S3 `user_{id}/…`. **Pas de middleware
ni de guard global.**

Relevé exhaustif (vérifié) : ✅ DeviceRegistryViewSet, ActuatorChannelConfigViewSet
(+actions), MetricsViewSet (isolation par index ES ; user_id passé mais **non inclus
dans la query** — garantie par le routage d'index côté client), LiveMetricsView,
BuildFirmwareView/FirmwareDownloadView/BuildRecords, UnitConversion/Formula/
FormulaImport/ConversionImport, ReportTemplate (Q(is_predefined)|Q(user)),
ReportConfiguration/Execution/Generate, FluidCatalog (prédéfinis OR user ;
prédéfinis réservés is_staff), DeviceListViewSet(etl), les 5 ViewSets sites
(+validation ownership dans perform_create, duplicate, linked_devices), authent.

### Failles / points faibles relevés (à corriger en Phase 3)
1. **CORS_ALLOW_ALL_ORIGINS = True**.
2. Pas de DEFAULT_PERMISSION_CLASSES — cas réel : `PredefinedDeviceListView` public
   (catalogue global, peut être volontaire mais à trancher explicitement).
3. `verify_aud: False` + aud "account" accepté → un token d'un autre client du
   même realm passe.
4. Pas de vérification d'issuer.
5. Pas de mapping rôles Keycloak → is_staff/is_superuser.
6. **Quotas Free incohérents selon le chemin** : signal = 3/1/0, authentication.py
   = 3/1/0, vues authent/devices = 5/2/1, fixture YAML = 3/1/0.
7. Isolation ES dépend uniquement du routage par index (query sans clause user_id).
8. HealthCheckMiddleware réécrit HTTP_HOST pour /health/*.

## 4. subscription
Modèles SubscriptionTier/UserProfile (→ models.md §5), **aucun endpoint**. Signal
post_save User → UserProfile + tier Free. Consommateurs : quotas devices
(devices/views.py:166-180), min_build_interval (firmware_builder/views.py:148).
Commande create_missing_profiles (backfill, --dry-run).

## 5. sites
Modèles + endpoints → models.md §4 et api-rest.md §7. Serializers avec validation
coordonnées appariées, contenu SVG, structure fields/linked_devices + ownership device.

## 6. health
- `GET /health/live/` → 200 `{"status":"ok","service":"og-device-hub"}` (csrf_exempt).
- `GET /health/ready/` → check DB (503 si KO) + cache Redis non critique
  (ok|degraded|unavailable).
- Middleware force HTTP_HOST=health-check-internal pour /health/* (bypass
  ALLOWED_HOSTS probes K8s) — doit rester premier.

## 7. bootstrap_db
`task init-db` : makemigrations (8 apps) → migrate → init_subscription → create_su →
init_device_cap → init_device_type → init_mcu → init_predefined_device →
init_fluid_catalog → init_global_conversions → init_global_formulas.
Fixtures YAML → models.md §10. Commandes idempotentes (update_or_create).

## 8. init_keycloak
- `scripts/create_keycloak_client.py` (698 l., interactif) : realm `pnex-realm`,
  client **confidentiel** `pnex`, standardFlow + directAccessGrants activés,
  implicitFlow off, serviceAccounts off, redirectUris `{DJANGO_URL}/*`,
  `access.token.lifespan=3600`, `pkce.code.challenge.method=S256`, scopes
  web-origins/profile/roles/email. Auto-répare la config client ; crée/répare des
  users test ; génère keycloak_credentials.json + env vars.
  **Aucun rôle realm/client spécifique créé.**
- Variante K8s : `create_keycloak_client_k8s.py` (non-interactive, secrets dans K8s).
- Scripts de test : test_keycloak_auth_flow.py, test_sso_api_auth.py,
  check_keycloak_config.py.
- ⚠️ Docs partiellement obsolètes (realm django-realm / client django-og-device-hub /
  URLs divergentes entre README, QUICK_REF et le script réel).

## 9. Variables d'environnement (.env.example existe mais sans vars Keycloak)

- **Django core** : DJANGO_SECRET_KEY, DJANGO_DEBUG, DJANGO_ALLOWED_HOSTS,
  DJANGO_CSRF_TRUSTED_ORIGINS_LIST, DJANGO_SUPERUSER_*
- **DB** : DATABASE_NAME/USER/PASSWORD/HOST/PORT
- **Keycloak** : KEYCLOAK_URL/REALM/CLIENT_ID/CLIENT_SECRET
- **Celery** : CELERY_BROKER_URL/RESULT_BACKEND + 3 intervalles beat
- **Channels/Redis** : CHANNELS_REDIS_URL, DEVICE_REDIS_CLUSTER_HOST/PORT/PASSWORD/DB
- **Firmware** : FIRMWARE_BUILD (argowf|k8s_job), FIRMWARE_POLL_INTERVAL,
  FIRMWARE_CLEANUP_INTERVAL_HOURS, FIRMWARE_BUILDER_IMAGE, FIRMWARE_GIT_REPO/BRANCH
- **K8s** : K8S_FIRMWARE_NAMESPACE, K8S_IN_CLUSTER, KUBECONFIG_PATH, K8S_JOB_*,
  K8S_IMAGE_PULL_SECRETS, K8S_SSH_SECRET ; contrôleurs K8S_CONTROLLER_* (à supprimer)
- **Argo** : ARGO_WF_HOST/PORT/SECURE/TOKEN/NAMESPACE (à supprimer)
- **S3** : S3_HOST/PORT/ENDPOINT_URL/ACCESS_KEY/SECRET_KEY/BUCKET/SECURE/
  SIGNATURE_VERSION/REGION — bucket unique `pnex`
- **NATS** : NATS_HOST/PORT
- **WS** : WEBSOCKET_DEVICE_TOKEN_VALIDATION_CACHE_TTL (10 s)
- **ES** : ELASTICSEARCH_HOST/PORT/USERNAME/PASSWORD/USE_SSL/VERIFY_CERTS/TIMEOUT,
  ELASTICSEARCH_BATCH_SIZE/FLUSH_INTERVAL/INDEX_PREFIX, ENABLE_REDIS_NOTIFICATIONS

## 10. Notes clés pour la Phase 3

- Reproduire : validation JWKS RS256 (aud account|client_id), JIT provisioning
  (preferred_username/email/given_name/family_name) + UserProfile Free, proxy
  token/refresh, redirect SSO PKCE.
- **Renforcer** l'isolation : middleware/guard global plutôt que par-viewset ;
  CORS explicites ; DEFAULT permission deny-all ; vérification iss + aud stricte ;
  quotas Free unifiés ; mapping rôles si besoin is_staff.
- Provisionner les composants par tenant (org OpenObserve, etc.) à la création user —
  équivalent du signal UserProfile.
