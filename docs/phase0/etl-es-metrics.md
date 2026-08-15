# Inventaire — ETL / Elasticsearch / metrics (pnex-server)

> Rapport d'exploration Phase 0. Cible : OpenObserve (pipelines VRL) + moteur
> de formules Rust (wasmtime) + CoolProp FastAPI externe.

**FAIT MAJEUR : CoolProp n'est PAS appelé via HTTP aujourd'hui** — c'est un import
Python in-process (`import CoolProp.CoolProp`). La migration suppose de remplacer
ces appels directs. Pas de `pypdf` ni `pint` (PDF = WeasyPrint, unités = modèle maison).

## 1. Modèles etl
→ détail complet dans `models.md` §3. Résumé : UnitConversion, Formula, FluidCatalog,
FormulaDataSource, FormulaImport, ConversionImport, ReportTemplate, ReportConfiguration,
ReportExecution, FluidPropertyGroup.

## 2. Moteur de formules (etl/utils.py) — 100% Python AST, pas de sandbox OS

- `SAFE_OPERATORS` (:12-22) : Add, Sub, Mult, Div, FloorDiv, Mod, Pow, USub, UAdd
- `SAFE_COMPARISONS` (:25-32) : Eq, NotEq, Lt, LtE, Gt, GtE
- `SAFE_FUNCTIONS` (:35-63) : abs, round, min, max, sum, pow + sqrt, exp, log, log10,
  log2, sin, cos, tan, asin, acos, atan, atan2, sinh, cosh, tanh, ceil, floor, trunc,
  degrees, radians
- `_init_coolprop_functions()` (:67-86) : au chargement du module, tente
  `from CoolProp.CoolProp import PropsSI` → injecte dans SAFE_FUNCTIONS ; idem
  `HAPropsSI` (air humide). **Import in-process, silencieusement ignoré si absent.**
- `SAFE_CONSTANTS` (:89-93) : pi, e, tau
- `CUSTOM_FUNCTIONS` + `register_custom_function()` (:96-127) : registre de fonctions
  custom — **jamais utilisé ailleurs dans le code**.
- `SafeEvaluator` (:130-276) : `ast.parse(mode="eval")` + descente récursive.
  Nodes : Constant/Num, Name, BinOp, UnaryOp, Compare chaîné, Call (safe+custom),
  IfExp ternaire, List/Tuple littéraux. Tout le reste → `ValueError: Unsupported
  expression type` (les Attribute type `__import__` sont rejetés).
- API : `safe_eval(expression, variables)` (:283-315), `validate_expression` (:318-346,
  tolère variables non définies), `extract_variables` (:349-382, NodeVisitor sur
  `ast.Name` — utilisé pour vérifier que chaque variable a une data source).

**Points d'appel** : views.py:419-446 (evaluate API), consumers.py:287-289 (WS),
tasks.py:214/277/443 (rapports), models.py:97-99 (conversions custom),
etl_compute.py:499-537 (event-driven, **injecte PropsSI/HAPropsSI directement
dans le context** :509-513).

**CoolProp — 5 points d'injection** :
1. etl/utils.py:70 (`PropsSI` dans SAFE_FUNCTIONS — expressions utilisateur :
   `PropsSI('H','T',temperature,'P',pressure,'Water')`, fluide = littéral string)
2. etl/utils.py:78 (`HAPropsSI` psychrométrie)
3. etl_compute.py:510 (ré-import direct)
4. models.py:383-386 (validation fluide `PropsSI("T","P",101325,"Q",0,name)`)
5. import_coolprop_fluids.py (import catalogue complet via `FluidsList`)

→ Le service FastAPI devra exposer `PropsSI(out, name1, val1, name2, val2, fluid)`
et `HAPropsSI(...)` avec la **même sémantique positionnelle** (les expressions en DB
les référencent par nom).

**Conversions d'unités** : appliquées **après agrégation** (views.py:344-354,
consumers.py:266-270, tasks.py:106-269) — sauf etl_compute.py:273-318 où la
conversion s'applique à la valeur brute de chaque mesure avant cache.

## 3. Endpoints REST etl
→ détail complet dans `api-rest.md` §8.

## 4. Génération PDF / charts

### Charts — etl/chart_generator.py (matplotlib 3.10.8, backend Agg)
Retour : PNG base64 data-URI (dpi 150). 7 générateurs : time_series (:19-91),
aggregation barres (:94-174), distribution box plot (:177-242), comparison (:245-322),
computed_expression (:325-395), individual_data_source (:398-470), cleanup (:473).

### PDF — etl/pdf_generator.py (WeasyPrint 67.0)
- HTML/CSS template **fixe codé en dur** (:14-336) (header, report-info, sections
  formules, aggregations grid, warnings, data sources, charts), A4,
  `page-break-inside: avoid`.
- `generate_pdf_report` (:339-385) : tout en mémoire → BytesIO.
- ⚠️ Le `layout` JSON de ReportTemplate n'est **pas consommé** — code mort ou
  fonctionnalité à implémenter réellement en cible.

### Orchestration — etl/tasks.py generate_report(execution_id)
max_retries=3, soft 300 s / hard 360 s, acks_late. Pipeline : fetch ES (size 10000)
→ agrégations → safe_eval → recalcul formule **à chaque timestamp via forward-fill
des variables** (:369-472) → charts → WeasyPrint → upload S3 `user_{id}/reports/report_{id}.pdf`
→ update ReportExecution.

### Stockage — etl/s3_storage.py (minio 7.2.20)
upload/download/delete/list/get_stat sur S3_CONFIG (bucket unique `pnex`).

## 5. Elasticsearch

### src/elasticsearch_client.py (elasticsearch 9.2.0)
- **Singleton**, 4 configs de connexion en cascade. ⚠️ password par défaut en dur (:61).
- **Nommage** : `get_user_index_name(user_id)` → **`user_{user_id}_measurements`**
  (:210-214). Un seul type d'index par user (architecture "unified").
- Redis optionnel pub/sub canal `sensor-updates` (flag ENABLE_REDIS_NOTIFICATIONS).
- Méthodes : `create_user_unified_indices`, `create_unified_index_template`
  (template `user_*_measurements`), `index_measurement`, `index_device_status`
  (doc id `{pred_dev}_{device_id}`), `search_measurements` (tri `@timestamp desc`),
  `get_latest_measurements`, `aggregate_measurements`, `delete_user_data` (GDPR),
  `index_unified_measurement` (normalise timestamp Unix/ISO), `migrate_legacy_to_unified`
  (scroll batch 1000).

### Mappings — src/elasticsearch_mappings.py
`UNIFIED_MEASUREMENTS_MAPPING` :
- Settings : `index.mode: "time_series"`, `routing_path: [device_id, metric_name,
  source_type]`, 1 shard / 0 replica, **ILM `sensor_data_policy`**, `refresh_interval: 30s`
- Dimensions (keyword) : `device_id`, `metric_name`, `source_type`
  (sensor/actuator/formula/etl), `pred_dev`
- Métrique : `value` double `time_series_metric: "gauge"`
- Métadonnées : `unit`, `tags` keyword
- GPS : `location` geo_point, `altitude` float, `location_source`,
  `location_device_id`, `gps_timestamp`
- Actuateur : `channel` int, `state` keyword, `pwm_value` int
- Formule : `formula_id`, `formula_name`, `input_devices` keywords
- `@timestamp` date `strict_date_optional_time||epoch_millis`, `created_at` date

### Batching — etl_nat_es_consumer.py
Batches **par user** (500 / 10 s), bulk API `refresh=False`, vérif erreurs item par
item, flush périodique 30 s + shutdown, création index à la volée.
Le doc formule ajoute : formula_id/name/type, property_index, variables (nested),
computed_at, `message_type: "formula_result"`, `source_type: "formula"`,
`pred_dev: "virtual_device"`.

### Retention — apply_retention_policy.py
⚠️ Boucle 300 s opérant sur le modèle **PostgreSQL** `metrics.Metrics`, PAS sur ES.
Retention dérivée de `user.profile.subscription_tier.data_retention`.
Pour ES : seule la référence ILM `sensor_data_policy` (gérée cluster-side).

## 6. App metrics

- Modèles Metrics/Ping = héritage PG ; **la lecture passe par ES**.
- `MetricsViewSet` (APIView get only) : `GET /api/v1/metrics/` — construit du query DSL
  (term device_id + term metric_name + range @timestamp), appelle
  `search_measurements(user.id, ...)`, remappe vers l'ancien format.
  ⚠️ Bug : lit `source.get("timestamp")` au lieu de `@timestamp` → `event_time` null.
- Pas de consumers dans l'app metrics — le WS dashboard est MetricsLiveConsumer (dev_ctl).

## 7. WS ws/etl/formulas/evaluate
→ détail dans `ws-channels-crypto.md` §2.4. Protocole : auth dans le 1er message,
commandes pause/resume/update_params, boucle 5 s, résultat formula_result.

## 8. Workers ETL event-driven (cœur de la migration OpenObserve)

### etl_compute.py — FormulaEventComputer
Pattern : **NATS sensors → compute → NATS formula results**.
- Subscribe `sensor.*.*.measurement.>` (queue group `fluid-compute-workers`).
- Cache formules rechargé 5 min ; map device_id → measurement → [formulas].
- Cache capteurs mémoire TTL 5 min ; constants initialisés une fois.
- À chaque mesure : conversion unité → update cache → si toutes variables
  présentes/non expirées → évaluation → publication.
- Publication : `etl.{uid}.{dev}.formula.{fid}.result.{idx}` avec payload complet
  (timestamp, user_id, device_id, pred_dev "virtual_device", metric_name (=nom
  formule [+ `_N` si multi-props]), value, source_type "formula", variables,
  computed_at). Résultat tuple → un message par propriété.

### etl_nat_es_consumer.py → ES (voir §5 batching)
### etl_nat_redis_consumer.py → Redis db 2 (FluidPropertyValueKey, TTL 24 h)

## 9. Dépendances Python par brique

| Brique | Libs |
|---|---|
| Moteur formules | stdlib ast/math/operator ; **CoolProp 7.2.0 in-process** |
| Agrégations | stdlib statistics |
| Conversions | modèle maison UnitConversion + safe_eval (pas de pint) |
| Elasticsearch | elasticsearch 9.2.0 |
| Charts | matplotlib 3.10.8 (Agg) |
| PDF | WeasyPrint 67.0 (pas de pypdf) |
| Stockage rapports | minio 7.2.20 |
| WS | channels 4.3.2, channels-redis, websockets |
| Workers NATS | nats-py 2.12.0, orjson |
| Redis | redis 7.1.0 (sync) + coredis 5.3.0 (async) |
| Tâches | celery 5.6.0 |
| API | Django 6.0, DRF 3.16.1, drf-spectacular, django-filter, cors-headers |
| Divers | pytz, pandas (présent non utilisé), httpx |

## 10. Points d'attention migration

1. **CoolProp in-process → FastAPI** : signatures positionnelles identiques
   (PropsSI/HAPropsSI), 5 points d'injection à remplacer.
2. **Moteur formules Rust** : contrat = `safe_eval` + `validate_expression` +
   `extract_variables` (whitelist opérateurs/fonctions/constantes, IfExp, listes,
   rejet Attribute).
3. **ES → OpenObserve** : contrat étroit — index par user, dimensions
   (device_id, metric_name, source_type), gauge value, bulk batch 500/10 s,
   recherche term+range tri timestamp desc. Prévoir l'équivalent ILM (retention
   par tier) côté OpenObserve.
4. Bug existant metrics/views.py:138 (`timestamp` vs `@timestamp`).
5. ReportTemplate.layout non consommé par le PDF — code mort à assumer ou
   fonctionnalité à implémenter.
