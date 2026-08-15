# Inventaire — API REST DRF (pnex-server)

> Rapport d'exploration Phase 0. Cible : parité de contrat en Loco/Axum.

## 1. Configuration globale

### settings.py
- **REST_FRAMEWORK** (:200-212) :
  - `DEFAULT_SCHEMA_CLASS` : drf_spectacular
  - **Auth par défaut** : 1. `KeycloakJWTAuthentication` (Bearer JWT), 2. DRF `TokenAuthentication` (`Authorization: Token <t>`), 3. SessionAuthentication
  - **Throttling** : AnonRateThrottle 5/min + UserRateThrottle 1000/min
- **PAGINATION : AUCUNE**. Les list = tableaux JSON bruts. Seuls `MetricsViewSet` et `LiveMetricsView` wrappent manuellement `{"count", "results"}`.
- **OpenAPI** : `GET /schema/`, `/schema/swagger-ui/`, `/schema/redoc/` (drf-spectacular).

### KeycloakJWTAuthentication (authent/authentication.py:16-226)
- `Authorization: Bearer <jwt>` ; validation RS256 via JWKS PyJWKClient, aud contrôlée manuellement contre `{account, client_id}`.
- **JIT-provisioning** : `get_or_create` sur `preferred_username`, sync email/noms, création UserProfile tier "Free".
- Erreurs 401 : "Token has expired" / "Invalid token: ...".

### Routage racine (device_hub/urls.py:28-48)
| Chemin | Include |
|---|---|
| `/admin/` | admin Django |
| `/api-token-auth/` | `obtain_auth_token` DRF |
| `/health/` | health.urls |
| `/api/v1/` | authent, devices, metrics, dev_ctl, firmware_builder, sites |
| `/api/v1/etl/` | etl.urls (DefaultRouter) |
| `/schema/`… | drf-spectacular |

Note : `format_suffix_patterns` sur devices/authent/firmware → routes `.json`/`.api` aussi.

## 2. Health (aucune auth, @csrf_exempt)
- `GET /health/live/` → 200 `{"status":"ok","service":"og-device-hub"}`
- `GET /health/ready/` → 200 `{"status":"ready","checks":{"database":"ok","cache":"ok|degraded|unavailable"}}` ; 503 si DB down.
- Middleware `HealthCheckMiddleware` bypass ALLOWED_HOSTS via host virtuel `health-check-internal`.

## 3. Authent (/api/v1/)

### `GET /api/v1/user-info/` — IsAuthenticated
```json
{"id":1,"username":"...","email":"...","first_name":"...","last_name":"...",
 "is_staff":false,"is_active":true,
 "profile":{"subscription_tier":"Free","max_sensor_devices":5,"max_actuator_devices":2,
            "max_mixed_devices":1,"language":"en","timezone":"UTC","date_format":"YYYY-MM-DD",
            "theme":"light","preferences":{},"grafana_url":null,
            "llm_endpoint_openapi_compatible":null,"llm_token":null,"llm_model":null},
 "device_count":{"total":0,"active":0,"by_type":{}}}
```
Auto-création profil Free si absent.

### `GET /api/v1/user/preferences/`
```json
{"default_device_filters":{"device_type":"all","active_only":false,"sort_by":"device_id","sort_order":"asc"},
 "notification_settings":{"email_notifications":true,"device_offline_alerts":true,"firmware_update_alerts":true},
 "display_preferences":{"theme":"light","timezone":"UTC","date_format":"YYYY-MM-DD","language":"en",
                        "grafana_url":null,"llm_endpoint_openapi_compatible":null,"llm_token":null,"llm_model":null},
 "custom":{}}
```

### `POST|PUT|PATCH /api/v1/user/preferences/update/`
Body tous optionnels : `language, timezone, date_format, theme, grafana_url, llm_endpoint_openapi_compatible, llm_token, llm_model, display_preferences (nested legacy), preferences (merge JSON), notification_settings, default_device_filters`.
Réponse : `{"message":"Preferences updated successfully","updated_fields":[...],"profile":{...}}`

### `GET /api/v1/user/device-statistics/`
`{"total_devices":n,"active_devices":n,"inactive_devices":n,"by_device_type":{...},"subscription_limits":{...}}`

### `POST /api/v1/oauth2/token/` — AllowAny
Proxy vers Keycloak token endpoint. Body password grant `{"grant_type":"password","username","password"}` ou authorization_code+PKCE `{"grant_type":"authorization_code","code","code_verifier","redirect_uri"}`.
Réponse : `{"access_token","token_type":"Bearer","expires_in":300,"refresh_token"}`. Erreurs 400 `{"error":"..."}`.

### `POST /api/v1/oauth2/refresh/` — AllowAny
`{"refresh_token"}` requis → nouveau couple access/refresh.

### `GET /api/v1/oauth2/test/` — IsAuthenticated
`{"message":"OAuth2 authentication successful","user":{...}}`

### `GET /api/v1/oauth2/sso/` — AllowAny
Query : `action` (register|reset), `code_challenge` (PKCE requis), `code_challenge_method` (S256), `redirect_uri`. **302** vers Keycloak (`kc_action=register|UPDATE_PASSWORD`).

### `POST /api-token-auth/`
DRF standard, backend EmailOrUsernameBackend (login email OU username), BCrypt. → `{"token":"..."}`.

## 4. Devices (/api/v1/)

### DeviceRegistryViewSet — IsAuthenticated (views.py:90-275)
**`GET /devices/`** — query : `device_type` (nom, `all` no-op), `capability`, `device_id` (exact), `active` (true|false). Liste non paginée.

**`POST /devices/`** — body `{"device_id" (requis), "predefined_device_name" (requis, 400 si inexistant), "metadata"?}` :
- Device déjà enregistré actif → **400** `{"detail":"This device is already registered and active."}`
- Device inactif → réactivation **200** `{"detail":"Device reactivated successfully."}`
- Quota tier atteint → **400** `{"detail":"Device limit reached for X devices in your subscription tier."}`
- Création : device `active=False` + DeviceToken auto (token_urlsafe(32) + clé ChaCha20). **201** :
```json
{"id":"...","user":1,"device_id":"...","metadata":null,"predefined_device_name":"...",
 "device_type":"sensor","capabilities":[{"id":1,"name":"...","mode":"input"}],"active":false,
 "device_token":{"token":"...","encryption_key":"...","is_active":true,"created":"ISO"},
 "allow_dynamic_measurements":false,"discovered_measurements":[],"max_unique_measurements":100}
```

**`GET /devices/{id}/`** (pk = DeviceRegistry.id)
**`PUT|PATCH /devices/{id}/`** — **seul `metadata` modifiable**, sinon 400 `{"detail":"Only metadata updates are allowed."}`
**`DELETE /devices/{id}/`** — supprime BuildRecords + DeviceToken. **204 avec body** `{"detail":"Device and its token removed successfully. Cleaned N firmware records."}`

### `GET /api/v1/device-capabilities/` — IsAuthenticated
Query `mode` (input|output|input_output). → `[{"id":1,"name":"read_temperature","mode":"input"}]`

### `GET /api/v1/predefined-devices/` — AllowAny
Query : `capabilities` (multi), `board`, `device_type`, `name`/`pretty_name` (icontains), `revision`.
→ `{name, pretty_name, prestashop_product_id, prestashop_buy_url, byod_doc_url, image_source_url, description, revision, device_type, capabilities[], board}`

### ActuatorChannelConfigViewSet — IsAuthenticated (views.py:278-449)
**`GET /actuator-channels/`** — filtres `channel_number`, `enabled`, `mode`, `actuator_device_id`. Tri `(actuator_device, channel_number)`.
**`POST /actuator-channels/`** — requis : `actuator_device_id` (write_only), `channel_number` (unique/device), `mode` (binary|pwm|follow).
- binary : `threshold` + `comparison` (lt|gt) requis
- pwm : `min_sensor_value` + `max_sensor_value` requis, min < max
- Optionnels : `enabled` (true), `sensor_input_name` (CSV `device:metric`), `aggregation_method` (mean|max|min|single), `invert_logic`, `hysteresis_seconds` (60), `hysteresis_value` (0), `min_pwm` (0)/`max_pwm` (255), `safe_mode` (off|on|keep)
- Validations : device type actuator|mixed, pas de doublon canal (400)
- **Side-effect** : publish NATS via Celery
- Réponse : `{id, actuator_device_id, channel_number, enabled, sensor_input_name, aggregation_method, mode, threshold, comparison, invert_logic, hysteresis_seconds, hysteresis_value, min_sensor_value, max_sensor_value, min_pwm, max_pwm, safe_mode, created_at, updated_at}`

**`GET|PUT|PATCH|DELETE /actuator-channels/{id}/`** — republish NATS à chaque mutation.
**`GET /actuator-channels/by_device/?device_id=X`** — 400 sans param, 404 si inconnu, liste triée par canal.
**`GET /actuator-channels/pod-status/{device_id}/`** — statut pod K8s → `{"device_id","exists","ready","replicas","ready_replicas","phase","pod_name"}`. **(SUPPRIMÉ en cible : pods K8s retirés)**

## 5. Metrics & live

### `GET /api/v1/metrics/` — MetricsViewSet (metrics/views.py:10-148, APIView get only)
Source ES index `user_{id}`. Query : `start_date`, `end_date` (ISO), `last_seconds` (int, 400 si non-int), `device_id`, `measurement` (term metric_name), `limit` (défaut 1000, fallback silencieux).
→ `{"count":n,"results":[{"user":1,"predefined_device_type":"...","device_id":"...","measurement":"...","value":1.23,"event_time":"..."}]}`
⚠️ Bug : lit `source.get("timestamp")` alors que l'index contient `@timestamp` → `event_time` probablement null.

### `GET /api/v1/live-metrics/` — LiveMetricsView (dev_ctl/views.py:12-169)
Source Redis db 2. Query : `device_id`, `sensor` (nom capability ; mapping = strip `read_`/`get_`).
→ `{"count":n,"results":[{"device_id","predefined_device","sensor","value","timestamp"}]}` ; `no_data` / `json_error` / `redis_error` par entrée.

## 6. Firmware builder (/api/v1/)

### `POST /build-firmware/` — IsAuthenticated
Body : requis `wifi_ssid` (≤100), `wifi_password` (≤100), `predefined_device_name` (≤100), `pnex_host` (≤200), `device_id` (≤100) ; optionnels `insecure` (0|1), `server_port` (443), `metadata`, `force_rebuild` (non utilisé côté serveur).
- 404 `{"error":"Device with ID '...' not found"}` ; 403 quota ; **429** intervalle min (défaut 15 min) `{"error":"Build interval not met for your subscription tier. Please wait before next build"}` ; 500 soumission
- **201** : `{"build_record_created":bool,"job_submitted":true,"backend":"k8s_job|argowf","job_name":"...","job_status":"Pending|...","message":"..."}`

### `GET /download/firmware/{device_id}/`
Stream binaire `application/octet-stream`, `Content-Disposition: attachment`. 404 si absent.

### `GET /build-records/`
Liste non paginée `{id, user, device_id, timestamp, success, argo_wf_job_name, build_phase, firmware_bin_s3_key}`.

### `DELETE /build-records/{pk}/`
- 400 si build réussi : `{"error":"Cannot delete successful firmware builds"}`
- 400 si device existe encore : `{"error":"Cannot delete firmware record while device still exists"}`
- sinon **204** `{"message":"Firmware record deleted successfully"}`

## 7. Sites (/api/v1/ — PK UUID, tous IsAuthenticated)

### SiteViewSet
- `GET /sites/` — filtres `tags` (multi AND), `has_coordinates` ; search name/description/address ; tri name/created_at/updated_at (défaut -created_at)
- `POST /sites/` — `{name (requis), description, latitude, longitude (les 2 ensemble sinon 400), address, tags[], metadata{}}` ; read-only id/user/diagram_count/created_at/updated_at
- `GET|PUT|PATCH|DELETE /sites/{uuid}/`
- `GET /sites/{uuid}/diagrams/`

### SVGFileViewSet
- CRUD `/svg-files/` — body `{filename (requis), name (requis), content (requis, doit contenir `<svg` avant `</svg>`), tags, metadata}` ; réponse inclut `usage_count` ; search name/filename ; filtre tags

### SiteDiagramViewSet
- CRUD `/site-diagrams/` — filtres `site`, `svg_file` ; tri display_order (défaut display_order, -created_at) ; **retrieve utilise DetailSerializer avec `svg_content`**
- Body `{site (uuid), svg_file (uuid), display_name, display_order, metadata}` — validation propriété user (400)
- `POST /site-diagrams/{uuid}/duplicate/` — body `{"target_site_id"}` → copie annotations + saved views, **201**

### AnnotationViewSet
- CRUD `/annotations/` — filtre `site_diagram` ; tri created_at/updated_at/x/y
- Body `{site_diagram (uuid), x, y (requis), title (requis), fields [{id,label,value,type:text|textarea|number|date}], linked_devices [{device_id (doit exister chez l'user sinon 400), device_name (auto), sensors[], linked_at (auto)}], zoom, pan_x, pan_y}`
- ⚠️ action `save_view` définie mais NON routée (endpoint mort)

### SavedViewViewSet
- CRUD `/saved-views/` — filtre `site_diagram` + `tags` (AND) ; body `{site_diagram, name (requis), zoom, pan_x, pan_y, tags}`

## 8. ETL (/api/v1/etl/ — DefaultRouter, IsAuthenticated)

### UnitConversionViewSet — CRUD `/unit-conversions/`
Body : `{name, from_unit, to_unit, conversion_type (linear|affine|custom), multiplier (1.0), offset (0.0), expression (requis si custom), description}`.
- `POST .../{id}/test/` — `{"test_values":[100,200]}` → `{"conversion":{...},"results":[{"input":100,"output":1450.38,"unit":"psi"}]}` ; 400 `{"error":"Conversion failed: ..."}`
- `POST .../import_conversion/` — `{"global_conversion_id":123,"custom_name":"..."}` → 201 import / 200 déjà importé / 404

### FormulaViewSet — CRUD `/formulas/`
Create/update avec **data_sources imbriqués** :
```json
{"name":"...","description":"...","formula_type":"simple_math|fluid_property|power_calculation|rate_of_change",
 "expression":"(temp_inlet - temp_outlet) * flow_rate * 4.186","result_unit":"W",
 "fluid_config":{"fluid":"Water"},"category":"","tags":[],
 "compute_on_event":false,"cache_ttl":60,"fluid_property_group":null,
 "data_sources":[{"source_type":"device","device":42,"measurement_name":"temperature","variable_name":"temp_inlet","unit_conversion":5,"order":0},
                 {"source_type":"constant","constant_type":"number","constant_value":"1.2","variable_name":"k"}]}
```
Validations : expression safe-eval ; toutes variables de l'expression doivent avoir un data source (400 `{"data_sources":"Missing data sources for variables: ..."}`) ; data_sources remplacés intégralement à l'update ; interdiction is_predefined=true.
Réponse GET ajoute `variable_names`, `is_editable`, `import_source`.

**`POST /formulas/{id}/evaluate/`** — body `{"start_time":"ISO","end_time":"ISO","aggregation":"raw|avg|min|max|sum"}` (défaut raw, mais raw tombe dans fallback mean !) :
```json
{"formula":{...},"result":123.4,"result_unit":"W","time_range":{"start","end"},
 "aggregation_method":"avg","data_sources":[{"variable","source_type","device_id","measurement","raw_values_count","aggregation","aggregated_value","unit_conversion","has_data"}],
 "variables_used":{"temp_inlet":20.1},"warnings":[],"status":"success|partial_data"}
```
Erreurs : 503 ES down, 400 (device/conversion/constante/évaluation), 500 fetch ES.

**`POST /formulas/import/`** — `{"global_formula_id":123,"custom_name":"...","copy_data_sources":true}` → 201/200/404. Les devices/conversions remis à null à l'import.

### GlobalFormulaViewSet — ReadOnly `/global-formulas/`
Filtres `category`, `tags` (AND), `search`. + `categories/`, `tags/`. Serializer avec `data_sources_template`, `variable_names`.

### GlobalConversionViewSet — ReadOnly `/global-conversions/` + categories/tags

### FormulaImportViewSet / ConversionImportViewSet — ReadOnly
`{id, original_formula(_name), user_formula(_name), overrides, receive_updates, imported_at, last_updated_at}`

### ReportTemplateViewSet — CRUD `/templates/`
Scope : prédéfinis OU à l'user. Modifier/supprimer un prédéfini → 403.
Body : `{name, description, page_size (A4|Letter), orientation (portrait|landscape), layout {sections:[{type:text|chart|table|spacer,...}]}}` ; réponse ajoute `is_predefined`, `is_editable`.
`POST .../{id}/clone/` — `{"name":"..."}` → **201**.
⚠️ Le layout n'est PAS consommé par le générateur PDF actuel (HTML fixe).

### ReportConfigurationViewSet — CRUD `/configurations/`
`{name, description, template (id), formulas [ids], time_range_type (hour|day|week|month), aggregation_config {}, timezone (pytz), text_variables {}, schedule {enabled,cron,email_to}|null, is_active}` ; réponse ajoute `template_name`, `formula_names[]`, `schedule_status`.

### ReportExecutionViewSet — ReadOnly + delete `/executions/`
`{id, configuration, configuration_name, status (pending|processing|completed|failed), start_time, end_time, file_path, file_size, processing_*, error_message, data_points_processed, download_url, created_at}`.
⚠️ `download_url` = `/api/reports/executions/{id}/download/` — **chemin historique incohérent** avec la vraie route `/api/v1/etl/executions/{id}/download/`.
`GET .../{id}/download/` — stream PDF. `DELETE .../{id}/` — 204.

### `POST /generate/`
`{"config_id":n,"start_time":"ISO","end_time":"ISO"}` (end > start sinon 400) → 201 ReportExecution + Celery generate_report.

### DeviceListViewSet — ReadOnly `/devices/` (préfixe etl !)
`{id, device_id, predefined_device_name, metadata, active, available_measurements:[{name, source: predefined|discovered, mode}]}`

### FluidCatalogViewSet — CRUD `/fluids/` + `categories/`
Scope : prédéfinis + fluides de l'user. Filtres `category`, `is_predefined`, `search`.
`{name, coolprop_name (sans espaces), category (water|refrigerant|air|hydrocarbon|cryogenic|mixture|other), description, chemical_formula, cas_number, min/max_temperature_k, min/max_pressure_pa, is_predefined, user_id, username}`.
Prédéfinis modifiables seulement par is_staff (403 sinon).

## 9. Apps sans endpoints
- **subscription** : models seulement (SubscriptionTier, UserProfile) + signal création profil.
- **bootstrap_db, simulator, k8s_ctl** : aucun endpoint.

## 10. Exemples requests/*.http

```http
### token.http
POST https://api.yourdomain.com/api-token-auth/
{"username":"admin@example.com","password":"...","client_id":"..."}   # client_id ignoré

### device.http
POST http://0.0.0.0:8000/api/v1/devices/     # Authorization: Token <t>
{"device_id":"device123","predefined_device_name":"soil_sensor"}

### metrics.http
GET http://0.0.0.0:8000/api/v1/metrics/?last_seconds=5&device_id=lab-1&measurement=soil_temperature

### build.http
POST http://0.0.0.0:8000/api/v1/build-firmware/
{"wifi_ssid":"coloc","wifi_password":"...","device_id":"dev-device-11",
 "predefined_device_name":"Soil Sensor","pnex_host":"api.yourdomain.com","metadata":"","force_rebuild":true}
GET https://api.yourdomain.com/api/v1/download/firmware/dev-device-11/

### emqx.http — ⚠️ endpoint /api/v1/emqx/authn N'EXISTE PLUS (héritage, ne pas reproduire)
```

## 11. Points d'attention pour Loco/Axum

1. **Pas de pagination** → tableaux nus ; metrics/live-metrics wrappent `{count, results}`.
2. **204 avec body JSON** (devices, build-records) — non standard, à trancher.
3. **Réactivation implicite** POST /devices/ (200 vs 201) — contrat important.
4. PUT/PATCH devices : **metadata uniquement**.
5. Suffixes `.json` sur devices/authent/firmware.
6. Quotas/throttling : 400 quota devices, 403 quota build, 429 min_build_interval.
7. `download_url` rapports incohérent — à corriger ou figer.
8. Endpoints morts : `save_view` (non routé), `emqx/authn` (fichier .http obsolète).
9. **3 schémas d'auth actifs** (JWT Keycloak JIT + DRF Token + Session). Bearer mal formé → pas de fallback ; token invalide → 401 immédiat.
10. Throttling anon 5/min sur les endpoints AllowAny (oauth2/token, refresh, sso, predefined-devices).
