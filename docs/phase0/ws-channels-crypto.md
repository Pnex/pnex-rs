# Inventaire — Couche WebSocket/Channels + Chiffrement (pnex-server)

> Rapport d'exploration Phase 0. Base pour la réécriture Loco/Axum WS
> (Phase 5 : ingestion ChaCha20 + broadcast desired-state config, SANS logique d'action).

## 1. Routing ASGI — `device_hub/asgi.py`

- `ProtocolTypeRouter` : `http` → Django ASGI, `websocket` → `AuthMiddlewareStack(URLRouter(...))` (asgi.py:39-46)
- `websocket_urlpatterns` (asgi.py:28-34) :

| Chemin WS | Consumer | Fichier |
|---|---|---|
| `ws/sensor/ingest` | `SensorIngest` | dev_ctl/views_ws.py:32 |
| `ws/actuator/cast` | `ActuatorCastConsumer` | dev_ctl/actuator_consumer.py:36 |
| `ws/metrics/live` | `MetricsLiveConsumer` | dev_ctl/views_ws.py:497 |
| `ws/etl/formulas/evaluate` | `FormulaEvaluationConsumer` | etl/consumers.py:18 |
| `ws/firmware/builds` | `FirmwareBuildConsumer` | firmware_builder/consumers.py:20 |

Config associée (settings.py) :
- `CHANNEL_LAYERS` : Redis `redis://…/1` (settings.py:280-287) — utilisé uniquement par FirmwareBuildConsumer (group_send Celery → WS)
- `DEVICE_REDIS` : host/port/password, **db 2** (settings.py:289-294)
- `NATS_CONFIG` : host:port, sans auth (settings.py:400-407)
- `WEBSOCKET_CONFIG.DEVICE_TOKEN_VALIDATION_CACHE_TTL` : 10 s par défaut, env `WEBSOCKET_DEVICE_TOKEN_VALIDATION_CACHE_TTL` (settings.py:417-425)

## 2. Consumers — détail

### 2.1 `SensorIngest` — `ws/sensor/ingest` (dev_ctl/views_ws.py:32-494)

**Authentification (query string)** — views_ws.py:44-53 :
- `?token=<base64(DeviceToken)>&device_id=<base64(device_id)>` — les DEUX paramètres sont **base64 puis décodés UTF-8**
- Lookup DB : `DeviceToken.objects.select_related("device", "device__predefined_device", "user").get(token=…)` (views_ws.py:145-155) ; `is_active` requis
- Vérif `token.device.device_id == device_id` sinon close **4006** "Token device mismatch" (views_ws.py:68-73)
- Clé de chiffrement du modèle : `token.encryption_key` ; absente → close **4008** (views_ws.py:80-84)
- Détection connexion dupliquée : clé Redis `device:ping:{user_id}:{device_id}` (`DevicePingKey`) ; ping âgé de < 12 s → close **4003** "Device already connected" (views_ws.py:102-119)
- Cache TTL 10 s : `validate_token_and_device_cached` (views_ws.py:157-180) existe mais **jamais appelée** — code mort, validation DB à chaque message (views_ws.py:341)

**Codes de fermeture** :
| Code | Cause | fichier:line |
|---|---|---|
| 4001 | AuthenticationFailed / erreur inattendue | views_ws.py:132-140 |
| 4002 | Pas de token | views_ws.py:141-143 |
| 4003 | Device déjà connecté (ping < 12 s) | views_ws.py:116-119 |
| 4004 | ValidationError | views_ws.py:135-137 |
| 4005 | Token/device invalide en cours de session | views_ws.py:337, 345-347 |
| 4006 | Token-device mismatch | views_ws.py:72 |
| 4008 | Pas de clé de chiffrement | views_ws.py:83 |

**Messages entrants (device → serveur)** — `receive()` views_ws.py:326-494, tout est **texte chiffré ChaCha20 base64** :
1. Validation token/device à chaque message (views_ws.py:331-347)
2. Déchiffrement : `decrypt_from_device(text_data.strip(), self.encryption_key)` (views_ws.py:356) ; échec → réponse chiffrée `"ERROR:decryption_failed"`
3. **PING standalone** : `decrypted.strip().upper() == "PING"` → ping Redis (TTL 86400 s) + réponse chiffrée `"PONG"` (views_ws.py:369-386)
4. **Format données** : `key=value` (split sur premier `=`) — sans `=` → `"error:invalid_format"` ; clé vide → `"error:empty_key"` ; nom > 100 chars → `"error:measurement_name_too_long"`
5. Validation capability (device non-dynamique : mesure doit exister dans `predefined_device.capabilities`) → `"error:invalid_capability:{msg}"` (views_ws.py:208-240, 421-431)
6. Tracking mesures découvertes (device dynamique : `discovered_measurements`, limite `max_unique_measurements`) → `"error:too_many_measurements"` (views_ws.py:242-279)
7. `key == "ping"` (format key=value) → ping Redis + réponse `"ok"`
8. Publication NATS (views_ws.py:471-486) :

```python
sensor_message = {
    "user_id": int, "username": email, "pred_dev": str,
    "device_id": str, "metric_name": key, "value": str,
    "timestamp": float(time.time()), "source_type": "sensor",
}
subject = TopicBuilder.sensor_measurement(user_id, device_id, key)
# → "sensor.{user_id}.{device_id}.measurement.{measurement}"
```
9. Réponse succès : chiffré `"ok"` ; exception globale → chiffré `"error"`

**Disconnect** : ferme NATS + Redis ; statut "inactive" géré par Celery (ping expire après 12 s).

### 2.2 `ActuatorCastConsumer` — `ws/actuator/cast` (dev_ctl/actuator_consumer.py:36-858)

**Auth = IDENTIQUE à SensorIngest** (token + device_id base64 query string) :
- 4002 si manquants (:59-74), lookup DeviceToken (:739-756), 4006 mismatch (:86-92), 4008 clé (:99-103)
- Type device validé : `predefined_device.device_type.name in ["actuator", "mixed"]` sinon close **4007** (:110-117)
- Duplicate ping < 12 s → 4003 (:127-145)
- Cache TTL : même pattern, code mort (:758-781, validation directe :232)

**Rôle** : pont bidirectionnel NATS ⇄ Protobuf ⇄ WS chiffré. Prototype du futur desired-state broadcast.

**Flux serveur → device (push)** :
- À la connexion : `send_initial_config()` (:629-671) — JSON config depuis `ActuatorChannelConfig` (`build_config_json` :673-737), JSON→Protobuf (`json_to_protobuf_config` :440-489), `SerializeToString()`, chiffre `binary_mode=True`, envoie texte base64.
- Sur changement : souscription NATS `actuator.{uid}.{dev}.config` → `on_nats_config` (:366-370, 379-406) — même pipeline.
- Données capteurs agrégées : souscription `actuator.{uid}.{dev}.sensor_data` → `on_nats_sensor_data` (:373-377, 408-438) — JSON `{timestamp, channels:[{channel, value, fresh}]}` → Protobuf `SensorData` → chiffré → WS.

**Flux device → serveur** (`receive()` :218-361) :
1. Validation token/device (4005)
2. Déchiffrement binaire ; 4 bytes "PING" → ping Redis + PONG chiffré (:247-278)
3. Parse `ActuatorState` Protobuf (:286-287)
4. Conversion docs ES unifiés (`protobuf_to_unified_elasticsearch_docs` :548-604) : un doc/canal — `metric_name="channel_{n}"`, `value` = 1.0/0.0/pwm/255, `source_type="actuator"`, champs `state`, `reason`, optionnels `pwm_value`, `sensor_value`, `threshold` ; timestamp = **heure serveur** (:561, 577)
5. Publication NATS de chaque doc au topic unifié `sensor.{uid}.{dev}.measurement.channel_{n}` (:298-333)
6. Publication legacy JSON au topic `actuator.{uid}.{dev}.state` (:336-340)
7. Échec déchiffrement → réponse chiffrée `"ERROR:decryption_failed"`

**Tracking connexion** : `ActuatorConnectionKey` — `actuator:{uid}:{dev}:connected` (=TTL 24 h) et `:last_seen` (:810-827).

### 2.3 `MetricsLiveConsumer` — `ws/metrics/live` (dev_ctl/views_ws.py:497-1097)

**Auth (query string, NON base64)** : `?token=<DRF token|JWT>&devices=<csv>&sensors=<csv>` (:515-529).
- JWT : `token.count(".") == 2` → Keycloak JWKS RS256, audience `account`/`pnex`, get_or_create user, fallback DRF token (:594-727)
- Ownership : chaque device doit appartenir à l'user sinon 4003 (:729-751)
- Codes : 4001 auth, 4002 token manquant, 4003 validation, 4000 inattendu

**Protocole sortant (JSON clair, PAS chiffré)** :
- Cache initial : dernières valeurs Redis db 2 (`SensorMeasurementKey`) avec `message_type: "cached_data"` (:864-962)
- Live : callback NATS → `{"type": "sensor_update", "data": {"device_id", "pred_dev", "sensor", "value", "timestamp", "message_type": "sensor_data"}}` (:1011-1044)

**Entrant** : `{"type": "ping"}` → `{"type": "pong"}` (:1046-1065)

**⚠️ BUG Django détecté** : souscriptions NATS en pattern **pluriel historique** `sensors.{user_id}.*.{device_id}.{sensor}` / `sensors.{user_id}.>.>` (:797-862) alors que l'ingestion publie sur `sensor.{uid}.{dev}.measurement.{name}`. **Ces sujets ne matchent pas** → le flux live ne reçoit rien via NATS ; seul `send_cached_data` fonctionne. À corriger dans la réécriture (souscrire via WildcardBuilder).

### 2.4 `FormulaEvaluationConsumer` — `ws/etl/formulas/evaluate` (etl/consumers.py:18-345)

**Auth dans le PREMIER message** (pas query string) — JSON clair :
```json
{"token": "...", "formula_id": 1, "aggregation": "avg", "time_window": 3600}
```
(:26-31, 103-155). DRF Token uniquement. Ownership formule vérifié. `connect()` accepte d'emblée.

**Commandes suivantes** : `{"command": "pause"|"resume"|"update_params"}`.

**Boucle** (asyncio 5 s) : requête ES par data source, agrégation avg/min/max/sum/std, conversion d'unité, `safe_eval(expression, variables)`. Réponse :
```json
{"type": "formula_result", "formula_id": …, "result": …, "result_unit": …, "timestamp": …,
 "time_range": {...}, "aggregation_method": …, "data_sources": [...], "variables_used": {...},
 "warnings": [...], "status": "success"|"partial_data"}
```
Erreurs : `{"type": "error", "error": "…"}`. Confirmation : `{"type": "connected", …}`.

### 2.5 `FirmwareBuildConsumer` — `ws/firmware/builds` (firmware_builder/consumers.py:20-360)

**Auth** : `?token=` query string ; DRF token OU JWT Keycloak (logique dupliquée de MetricsLive :191-337). Codes 4001/4002/4000.

**Mécanique** : channel layer Redis, groupe `firmware_builds_user_{user_id}` ; Celery pousse via `group_send`, handler `firmware_build_update` (:154-189) renvoie :
```json
{"type": "build_status", "build_id": …, "device_id": …, "status": "Running|Succeeded|Failed|Deleted",
 "success": bool, "argo_wf_job_name": …, "firmware_bin_s3_key": …, "timestamp": …, "message": "..."}
```

## 3. Chiffrement — `src/crypto_utils.py`

**Schéma : ChaCha20 (RFC 7539) via pycryptodome — SANS Poly1305 (pas d'AEAD, pas de MAC).**
⚠️ **Divergence avec migration.md §3** qui mentionne « ChaCha20-Poly1305 » : le code actuel n'est PAS AEAD.

| Élément | Détail | fichier:line |
|---|---|---|
| Clé | 32 octets `os.urandom`, stockée **base64** | crypto_utils.py:15-27 |
| Nonce | 12 octets aléatoires **par message** | crypto_utils.py:53 |
| Format wire | `base64( nonce(12) ‖ ciphertext )` en **texte WS** | crypto_utils.py:71-72 |
| Dérivation | **AUCUNE** (pas de KDF/HKDF/salt) | crypto_utils.py:15-27 |
| `encrypt_for_device(pt, key_b64, binary_mode=False)` | bytes ou str ; retourne base64 str | crypto_utils.py:30-81 |
| `decrypt_from_device(ct_b64, key_b64, binary_mode=False)` | valider len(key)==32, len(combined)>=12 | crypto_utils.py:84-136 |

**Stockage** : `DeviceToken.encryption_key` CharField(64) base64 (devices/models.py:122-127), auto-générée au save ; `DeviceToken.token` = `secrets.token_urlsafe(32)` ; unique (user, device). Injectée dans les builds firmware en env base64.

**Pour Axum** : crate `chacha20` (RustCrypto) mode non-AEAD, nonce 12 o préfixe, base64 standard paddé. Pas d'authentification de ciphertext → migration possible vers Poly1305 à prévoir (décision à prendre).

## 4. Protobuf — `actuator_message.proto` (proto3)

**Server → Device :**
- `ActuatorConfig` : `device_id(string,1)`, `timestamp(uint32,2)`, `repeated ChannelConfig channels(3)`, `data_timeout_seconds(uint32,4, défaut 10)`
- `ChannelConfig` : `number(1)`, `enabled(2)`, `mode(3)`, `safe_mode(5)` ; binaire : `threshold(6,float)`, `comparison(7)`, `hysteresis_seconds(8)`, `hysteresis_value(13,float)`, `invert_logic(14,bool)` ; PWM : `min_sensor_value(9)`, `max_sensor_value(10)`, `min_pwm(11)`, `max_pwm(12)`. **Champ 4 (`sensor_input`) REMOVED 2025** — l'ESP32 ne connaît plus les noms de capteurs, le serveur agrège.
- `SensorData` : `timestamp(1)`, `repeated ChannelData(2)` ; `ChannelData` : `channel(1, 1-4)`, `value(2, float agrégée serveur)`, `fresh(3, bool)`

**Device → Server :**
- `ActuatorState` : `device_id(1)`, `timestamp(2)`, `repeated ChannelState(3)`
- `ChannelState` : `number(1)`, `state(2)`, `pwm_value(3)`, `reason(4)`, `sensor_value(5)`, `threshold(6)`

**Enums** : `ChannelMode {BINARY=0, PWM=1, FOLLOW=2}` ; `Comparison {LESS_THAN=0, GREATER_THAN=1}` ; `SafeMode {OFF=0, ON=1, KEEP=2}` ; `ChannelStateValue {OFF=0, ON=1, PWM=2}` ; `StateReason {BELOW, ABOVE, PWM_CALCULATED, HYSTERESIS_ACTIVE, STALE_SENSOR_DATA, TIMEOUT, DISABLED, SAFE_MODE}`.

**Mappings JSON** : mode `"binary"/"pwm"/"follow"`, safe_mode `"off"/"on"/"keep"`, comparison `"lt"/"gt"`, state `"off"/"on"/"pwm"`, reason 8 chaînes snake_case (actuator_consumer.py:455-477, 606-627).

## 5. Topics NATS — `src/nats_topics.py`

### TopicBuilder
| Méthode | Pattern | line |
|---|---|---|
| `sensor_measurement(uid, dev, meas)` | `sensor.{uid}.{dev}.measurement.{meas}` | :26-33 |
| `actuator_config(uid, dev)` | `actuator.{uid}.{dev}.config` | :36-43 |
| `actuator_sensor_data(uid, dev)` | `actuator.{uid}.{dev}.sensor_data` | :46-53 |
| `actuator_state(uid, dev, ch?)` | `actuator.{uid}.{dev}.state[.{ch}]` | :56-65 |
| `actuator_command(uid, dev, ch?)` | `actuator.{uid}.{dev}.command[.{ch}]` | :68-77 |
| `etl_formula_result(uid, dev, fid, idx)` | `etl.{uid}.{dev}.formula.{fid}.result.{idx}` | :80-92 |

### TopicParser : `parse_sensor` (≥5 seg), `parse_actuator` (≥4 seg), `parse_etl` (≥7 seg) — :95-179.

### WildcardBuilder
`all_for_user`, `all_for_device`, `all_measurements` (`sensor.*.*.measurement.>`), `all_actuator_configs`, `all_actuator_states`, `all_formula_results` — :182-270.

**Consommateur réel** : `consume_nats_to_redis.py` souscrit `sensor.*.*.measurement.>` (queue group `redis-workers`), écrit Redis db 2 clé `SensorMeasurementKey`, TTL 24 h (:194-219, 126-192).

## 6. Redis keys — `dev_ctl/redis_models.py`

| Classe | Clé | Usage |
|---|---|---|
| `DevicePingKey` | `device:ping:{uid}:{dev}` | heartbeat TTL 24 h, duplicate < 12 s |
| `SensorMeasurementKey` | `{username}:{pred_dev}:{dev}:{sensor}` | dernière valeur dashboard |
| `ActuatorStateKey` | `{username}:{pred_dev}:{dev}:{channel}` | legacy |
| `ControllerDesiredStateKey` | `…:{dev}:desired_state` | desired state contrôleur |
| `ActuatorChannelStateKey` | `actuator:{uid}:{dev}:channel_{n}:state` | état rapporté |
| `ActuatorChannelTargetKey` | `actuator:{uid}:{dev}:channel_{n}:target` | cible commandée |
| `ActuatorConfigKey` | `actuator:{uid}:{dev}:config` | cache config |
| `ActuatorConnectionKey` | `actuator:{uid}:{dev}:connected` / `:last_seen` | statut connexion |
| `FluidProperty*Key` | `etl:fluid_state:…` etc. | ETL |

## 7. `publish_actuator_config` — devices/tasks.py:280-396 — base du futur desired-state

- Task Celery `publish_actuator_config(device_id, user_id)`, retry x3 backoff 5·2^n
- Déclencheurs : `post_save`/`post_delete` sur `ActuatorChannelConfig` (signals — publish TOUJOURS, quel que soit `enabled`) + hooks REST `perform_create/perform_update/perform_destroy` (devices/views.py:317-370)
- Message publié sur `actuator.{uid}.{dev}.config`, JSON clair (NATS interne non chiffré) :

```json
{
  "device_id": "…", "timestamp": <int epoch>, "data_timeout_seconds": 10,
  "channels": [
    {"number": 1, "enabled": true, "mode": "binary|pwm|follow",
     "sensor_input": "<csv device:metric>",   // encore dans le JSON (tasks.py:344) mais SUPPRIMÉ côté protobuf
     "safe_mode": "off|on|keep",
     "threshold": 0.0, "comparison": "lt|gt", "invert_logic": false,
     "hysteresis_seconds": 0, "hysteresis_value": 0.0,
     "min_sensor_value": 0.0, "max_sensor_value": 100.0, "min_pwm": 0, "max_pwm": 255}
  ]
}
```

- Chaîne : REST/signals → Celery → NATS config → `ActuatorCastConsumer.on_nats_config` → Protobuf → ChaCha20 → WS → ESP32.
- Fallback `send_initial_config` au (re)connect garantit la desired-state.

## 8. ACTION vs COLLECTE/BROADCAST (Phase 5)

### À GARDER (collecte + broadcast config)
| Élément | fichier:line |
|---|---|
| `SensorIngest` (ingestion chiffrée key=value → NATS) | dev_ctl/views_ws.py:32-494 |
| Ping/PONG chiffré + heartbeat Redis | views_ws.py:369-386 ; actuator_consumer.py:247-278 |
| `TopicBuilder.sensor_measurement` + publication | views_ws.py:484-486 |
| `consume_nats_to_redis` (cache dashboard) | dev_ctl/management/commands/ |
| `MetricsLiveConsumer` (modulo fix sujets `sensors.` → `sensor.*.measurement.>`) | views_ws.py:497-1097 |
| `ActuatorCastConsumer` partie CONFIG (souscription config, `send_initial_config`, `json_to_protobuf_config`, push chiffré) | actuator_consumer.py:366-737 |
| `publish_actuator_config` + signals + hooks REST | devices/tasks.py:280-396 ; signals.py:24-43 ; views.py:317-370 |
| `ActuatorCastConsumer.receive` partie STATE (réception `ActuatorState` → NATS) | actuator_consumer.py:280-343 |
| `FormulaEvaluationConsumer`, `FirmwareBuildConsumer` | etl/consumers.py ; firmware_builder/consumers.py |
| `crypto_utils.py`, `actuator_message.proto`, `nats_topics.py`, `redis_models.py` (hors clés contrôleur) | src/* |

### À SUPPRIMER (logique d'action / contrôle / K8s par actuateur)
| Élément | fichier:line |
|---|---|
| `deploy_compute_pod` / `remove_compute_pod` | devices/tasks.py:22-79 |
| `get_pod_status` / `list_all_pods` | devices/tasks.py:82-132 |
| `reconcile_actuator_pods` | devices/tasks.py:135-277 |
| `auto_deploy_compute_pod` / `cleanup_compute_pod` (signals — partie pod SEULEMENT) | devices/signals.py:45-101 |
| `run_compute_controller` (lit capteurs, agrège, publie `sensor_data` + COMMANDS) | devices/management/commands/run_compute_controller.py |
| `control_actuator` (commande manuelle) | dev_ctl/management/commands/control_actuator.py |
| `K8sControllerManager` (k8s_ctl/k8s_client.py, 326 l.) + app `k8s_ctl` | k8s_ctl/ |
| Flux `on_nats_sensor_data` du consumer (n'a plus de producteur sans pods) | actuator_consumer.py:373-438 |
| Topics `actuator_command`, `ControllerDesiredStateKey`, `ActuatorChannelTargetKey` | nats_topics.py:68-77 ; redis_models.py:37-45, 61-71 |

### Pièges pour la réécriture
1. **ChaCha20 nu (pas Poly1305)** — nonce 12 o/message, wire = `base64(nonce‖ct)` frame texte WS.
2. **Auth device** : token + device_id en query string, **double base64**.
3. Cache de validation TTL 10 s **mort** en Django — décision à prendre en Axum.
4. **Sujet dashboard incohérent** : `sensors.*` vs émission `sensor.*.*.measurement.*` — à unifier.
5. `sensor_input` dans le JSON config mais **ignoré** en Protobuf — le desired-state effectif ESP32 n'inclut pas les noms de capteurs.
6. Timestamps : le serveur **écrase** le timestamp device (actuator_consumer.py:544, 561) ; ingestion capteur timestampée à la réception.
7. Duplicate-connexion : fenêtre 12 s sur ping Redis, TTL 24 h.
