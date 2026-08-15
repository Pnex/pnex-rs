# Inventaire — Couche modèles Django (pnex-server)

> Rapport d'exploration Phase 0. Cible : SeaORM/PostgreSQL.

## 1. Vue d'ensemble

| App | Modèles | Notes |
|---|---|---|
| `devices` | 7 | Cœur du domaine IoT |
| `etl` | 10 | Conversions, formules, fluides, rapports |
| `sites` | 5 | SVG, annotations (PK UUID) |
| `subscription` | 2 | Tiers + profil utilisateur |
| `metrics` | 2 | Données de mesure (high volume, héritage PG) |
| `firmware_builder` | 1 | BuildRecord |
| `dev_ctl`, `k8s_ctl`, `bootstrap_db`, `authent`, `health` | 0 | vides / pas de models |
| `simulator` | 0 | commande sim_sensors seulement |

`User` Django natif (PK bigint auto, `BigAutoField`). CockroachDB : **plus aucun support actif** — cible réelle déjà PostgreSQL.

## 2. devices/models.py

### DeviceType (:8)
`name` CharField(100) unique.

### DeviceCapability (:15)
`name` CharField(255) unique ; `mode` choices input/output/input_output, default input.

### MCUBoard (:29)
`name` CharField(255) ; `soc` CharField(255) default "esp32" ; `details` JSONField null.

### PredefinedDevice (:38)
`name` (unique), `pretty_name`, `revision` (50), `device_type` FK CASCADE, `capabilities` M2M, `board` FK MCUBoard CASCADE, `device_doc_url`/`prestashop_product_id` (unique null)/`prestashop_buy_url`/`byod_doc_url`/`image_source_url`/`stl_files_url` URLField(1024), `description` Text.

### DeviceRegistry (:61)
| Champ | Type | Attr |
|---|---|---|
| user | FK User | CASCADE |
| device_id | CharField(255) | — |
| metadata | JSONField | null |
| predefined_device | FK | CASCADE, related `device_registries` |
| active | Bool | default False |
| allow_dynamic_measurements | Bool | default True |
| discovered_measurements | JSONField | default dict (mesure→true) |
| max_unique_measurements | Int | default 100 |

- `unique_together ("user", "device_id")` (:84-85)
- **save() métier** (:87-106) : création ou update de `predefined_device` → `allow_dynamic_measurements = (predefined_device.name ∈ {custom_sensor, custom_device})`.

### DeviceToken (:112)
| Champ | Type | Attr |
|---|---|---|
| user / device | FK | CASCADE |
| token | CharField(255) | unique |
| encryption_key | CharField(64) | null (ChaCha20 base64, 32 o) |
| is_active | Bool | default True |
| created | DateTime | auto_now_add |

- `unique_together ("user", "device")`
- **save() métier** (:135-144) : auto-génère `token = secrets.token_urlsafe(32)` et `encryption_key = generate_device_key()` si vides. **→ hook before_save SeaORM.**

### ActuatorChannelConfig (:150)
| Champ | Type | Attr |
|---|---|---|
| actuator_device | FK DeviceRegistry | CASCADE, limit actuator/mixed (admin/DRF only, pas SQL) |
| channel_number | Int | — |
| enabled | Bool | True |
| sensor_input_name | CharField(2000) | CSV capteurs |
| aggregation_method | choices mean/max/min/single | default single |
| mode | choices binary/pwm/follow | default binary |
| threshold | Float | null |
| comparison | choices lt/gt | null |
| invert_logic | Bool | False |
| hysteresis_seconds | Int | 60 |
| hysteresis_value | Float | 0.0 |
| min_sensor_value / max_sensor_value | Float | null |
| min_pwm / max_pwm | Int | 0 / 255 |
| safe_mode | choices off/on/keep | off |
| created_at / updated_at | auto | — |

- `unique_together [["actuator_device","channel_number"]]` + index redondant.
- **clean()** (:277-302) : type device actuator|mixed ; binary exige threshold+comparison ; pwm exige min/max avec min<max.

## 3. etl/models.py

### UnitConversion (:9)
`user` FK null=global ; `name`(255) ; `from_unit`/`to_unit`(50) ; `conversion_type` linear/affine/custom ; `multiplier` 1.0 ; `offset` 0.0 ; `expression` Text (Python avec `x`) ; `description` ; champs globaux `is_predefined`, `global_id` UUID (**non unique ici**), `version`, `category`(50), `tags` JSON list, `import_count` ; `created_at/updated_at`.
- `unique_together` **2 tuples** : `("user","from_unit","to_unit")` ET `("is_predefined","from_unit","to_unit")` → en PG, NULL user casse l'unique → **index uniques partiels** côté SeaORM.
- **convert(value)** (:89-100) : linear `x*m`, affine `x*m+o`, custom `safe_eval(expression, {"x": value})`.
- **clean()** : prédéfini ⇒ pas de user + catégorie requise.

### Formula (:117)
`user` FK null=global ; `name` ; `description` ; `formula_type` simple_math/fluid_property/power_calculation/rate_of_change ; `expression` Text ; `result_unit` Text (CSV si multi) ; `fluid_config` JSON `{fluid, props}` ; globaux `is_predefined`, `global_id` UUID **unique**, `version`, `category`, `tags`, `import_count` ; `compute_on_event` Bool ; `fluid_property_group` FK SET_NULL ; `cache_ttl` 60 ; `last_computed_at` ; timestamps.
- **clean()** (:222-255) : `compute_on_event` + fluid_property ⇒ group requis ; autres types ⇒ ≥1 data source device.

### FluidCatalog (:258)
`user` null=prédéfini ; `name`(100) ; `coolprop_name`(200) ; `category` water/refrigerant/air/hydrocarbon/cryogenic/mixture/other ; `is_predefined` ; `description` ; `chemical_formula`(50) ; `cas_number`(50) ; bornes min/max_temperature_k, min/max_pressure_pa (Float null).
- 2 unique_together : `("user","coolprop_name")`, `("is_predefined","coolprop_name")`.
- **is_valid_for_coolprop** (:375-393) : appelle `CP.PropsSI("T","P",101325,"Q",0,name)` **in-process**.

### FormulaDataSource (:420)
`formula` FK CASCADE related `data_sources` ; `source_type` device/constant ; `device` FK DeviceRegistry CASCADE null ; `measurement_name`(255) ; `constant_type` number/string/boolean ; `constant_value` Text ; `variable_name`(100) ; `unit_conversion` FK SET_NULL ; `order` 0.
- `unique_together ("formula","variable_name")`.
- **clean()** : device ⇒ device+measurement requis, efface constants ; constant ⇒ type+valeur requis, efface device.
- Pour CoolProp, les noms de fluides sont des **littéraux dans l'expression** : `PropsSI('H','T',t,'P',p,'Water')`.

### FormulaImport (:562) / ConversionImport (:604)
`user`, `original_formula|conversion`, `user_formula|conversion` (CASCADE), `overrides` JSON, `receive_updates` True, `imported_at`, `last_updated_at`. Uniques `(user, original, user_*)`.

### ReportTemplate (:649)
`name`, `description`, `is_predefined`, `user` null ; `page_size` A4/Letter ; `orientation` portrait/landscape ; **`layout` JSON requis** `{header, footer, sections:[{type: text|chart|table|spacer, ...}]}` — ⚠️ non consommé par le générateur PDF actuel.

### ReportConfiguration (:714)
`user`, `name`, `description`, `template` FK **PROTECT**, `formulas` M2M, `time_range_type` hour/day/week/month, `aggregation_config` JSON, `timezone` (pytz), `text_variables` JSON, `schedule` JSON null `{enabled, cron, email_to}`, `is_active`.

### ReportExecution (:804)
`configuration` FK CASCADE, `status` pending/processing/completed/failed, `start_time`/`end_time`, `file_path`(512, chemin S3), `file_size` BigInt, `processing_started_at`/`processing_completed_at`, `error_message`, `data_points_processed`, `created_at`. Index `(configuration, -created_at)`, `(status, -created_at)`.

### FluidPropertyGroup (:867)
`name`(50) unique, `display_name`(100), `group_type` basic_thermo/specific_heat/transport/psychrometric/all_thermo/custom, `property_codes` JSON list (codes PropsSI/HAPropsSI), `description`, `default_fluid`(100), `cache_ttl` 60, `is_predefined`.
Groupes prédéfinis : basic_thermo [H,S,D,U,Q], specific_heat [C,O], transport [V,L,A], psychrometric_basic/full, all_thermo.

## 4. sites/models.py (PK UUID partout)

### Site (:8)
UUID PK ; `user` FK ; `name` ; `description` ; `latitude`/`longitude` Decimal(9,6) null (validateurs ±90/±180, appariés) ; `address` Text ; `tags` JSON list ; `metadata` JSON dict ; `default_zoom` Decimal(5,2) 1.0 ; `default_pan_x/y` Decimal(10,2). Index `(user, name)`, `created_at`.

### SVGFile (:95)
UUID PK ; `user` ; `filename`(255) ; `name`(255) ; `content` Text (SVG XML brut) ; `tags`/`metadata`.

### SiteDiagram (:136)
UUID PK ; `site` FK CASCADE ; `svg_file` FK CASCADE ; `display_name` ; `display_order` 0 ; `metadata`. `unique_together [["site","svg_file"]]` + index `(site, display_order)`.

### Annotation (:178)
UUID PK ; `site_diagram` FK CASCADE ; `x`/`y` Decimal(10,2) requis ; `title`(255) ; `fields` JSON list `[{id,label,value,type}]` ; **`linked_devices` JSON list `[{device_id, device_name, sensors[], linked_at}]` — lien dénormalisé SANS FK** ; `zoom`/`pan_x`/`pan_y`.

### SavedView (:243)
UUID PK ; `site_diagram` FK ; `name` ; `zoom`/`pan_x`/`pan_y` ; `tags`. Index `(site_diagram, created_at)`.

## 5. subscription/models.py

### SubscriptionTier (:6)
`name`(100) ; `max_sensor_devices`/`max_actuator_devices`/`max_mixed_devices` Int requis ; `min_build_interval` DurationField (INTERVAL PG) défaut 15 min ; `data_retention` DurationField.

### UserProfile (:22)
`user` OneToOne CASCADE related `profile` ; `subscription_tier` FK **SET_NULL** ; `language`(10) "en" ; `timezone`(50) "UTC" ; `date_format`(20) ; `theme` light/dark/auto ; `preferences` JSON dict ; `grafana_url`/`llm_endpoint_openapi_compatible` URL(500) null ; `llm_token`(500) null ; `llm_model`(100) null.

## 6. metrics/models.py (héritage PG, lecture passe désormais par ES)

### Metrics (:6)
`user` FK ; `predefined_device_type`(255) ; `device_id`(255) **sans FK** (dénormalisé) ; `measurement`(255) ; `value` Float ; `event_time` DateTime. 5 index nommés `idxm_*`.

### Ping (:34)
Idem sans measurement/value. Index `idxp_*`.

## 7. firmware_builder/models.py

### BuildRecord (:5)
`user` FK ; `device_id`(255) null ; `timestamp` DateTime **auto_now** (update à chaque save) ; `success` False ; `argo_wf_job_name`(255) null ; `build_phase`(255) null ; `firmware_bin_s3_key`(255) NOT NULL default sentinelle `"default_for_migration"`.
- **save() métier** (:18-27) : génère `user_{id}/firmware/{device_id}-firmware.bin` si vide/sentinelle.

## 8. Signals (à porter en hooks/triggers)

| Signal | Handler | Logique |
|---|---|---|
| post_save ActuatorChannelConfig | `auto_deploy_compute_pod` (devices/signals.py:24-70) | TOUJOURS publish config NATS ; 1re config enabled → deploy pod (partie pod : SUPPRIMER) |
| post_delete ActuatorChannelConfig | `cleanup_compute_pod` (:73-102) | plus aucune config enabled → remove pod (SUPPRIMER) |
| post_save User | `create_user_profile` (subscription/signals.py:13-32) | UserProfile + tier Free à la création user — **critique, hook SeaORM** |
| post_save User | `save_user_profile` (:35-43) | re-save profil |

## 9. JSONFields → JSONB (récap)

MCUBoard.details ; DeviceRegistry.metadata, discovered_measurements ; UnitConversion.tags (**index btree JSONB** → GIN en SeaORM) ; Formula.fluid_config, tags (index) ; FormulaImport/ConversionImport.overrides ; ReportTemplate.layout ; ReportConfiguration.aggregation_config, text_variables, schedule ; FluidPropertyGroup.property_codes ; Site/SVGFile/SavedView.tags, metadata ; SiteDiagram.metadata ; Annotation.fields, linked_devices ; UserProfile.preferences.

## 10. Fixtures YAML bootstrap_db/data/

| Fichier | Contenu | Volume |
|---|---|---|
| devices/device_type.yaml | sensor, actuator, mixed, wifi_mesh, power_supply | 5 |
| devices/mcu.yaml | esp32-wroom-32, esp8266, esp32-c3 | 3 |
| devices/device_cap.yaml | capabilities input/output | 22 |
| devices/predefined_device.yaml | soil_sensor, custom_sensor, custom_device, 4_chan_relay (+prestashop ids) | 4 |
| subscriptions/subscription.yaml | Free, Basic, Pro, Enterprise, Ultimate, Admin | 6 |
| conversions/global/*.yaml | 9 catégories | 66 |
| formulas/global/*.yaml | 6 catégories | 39 |
| fluids/common_fluids.yaml | FluidCatalog CoolProp | 26 |

Commandes init idempotentes (update_or_create) : init_device, init_device_type, init_device_cap, init_mcu, init_predefined_device, init_subscription, init_global_conversions, init_global_formulas, init_fluid_catalog (+ etl : init_fluid_property_groups, import_coolprop_fluids, init_report_templates).
⚠️ `predefined_device.yaml` référence le board `generic` absent de mcu.yaml — créé à la volée par get_or_create.

## 11. DB (settings.py:126-148)

- DEBUG → SQLite ; sinon PostgreSQL (DATABASE_* env). CockroachDB : mention historique seulement.
- `DEFAULT_AUTO_FIELD = BigAutoField` → PK bigint auto partout sauf sites (UUID).

## 12. Points d'attention SeaORM

1. save() → hooks : DeviceRegistry, DeviceToken, BuildRecord.
2. clean() → validation service : ActuatorChannelConfig, UnitConversion, Formula, FluidCatalog, FormulaDataSource, ReportTemplate, ReportConfiguration, FluidPropertyGroup.
3. Signals → hooks/triggers : création UserProfile ; publish config NATS (+ pods K8s à supprimer).
4. FK SET_NULL/PROTECT à mapper : fluid_property_group, unit_conversion, subscription_tier (SetNull), ReportConfiguration.template (Restrict).
5. Doubles unique_together conditionnels (UnitConversion, FluidCatalog) → index uniques partiels.
6. metrics.Metrics/Ping volumineuses dénormalisées — partitionnement éventuel ; noms d'index idxm_*/idxp_*.
7. Annotation.linked_devices : JSON sans intégrité référentielle.
8. Migrations Django existantes : devices 0001-0003, etl 0001, firmware_builder 0001, metrics 0001, sites 0001, subscription 0001-0002 — schéma dérivable directement des models.
