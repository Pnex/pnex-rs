# Inventaire — Infra asynchrone & builds (pnex-server)

> Rapport d'exploration Phase 0. Cible : worker Loco sur queue Postgres
> (suppression Redis/Celery/Argo Workflows), firmware-builder crate Rust,
> suppression pods K8s par actuateur.

## 1. Celery

### Settings (`device_hub/settings.py`)
| Setting | Ligne | Valeur |
|---|---|---|
| `CELERY_TIMEZONE` | 214 | UTC |
| `CELERY_TASK_TIME_LIMIT` | 216 | 30 min |
| `CELERY_BROKER_URL` / `RESULT_BACKEND` | 217-218 | Redis db 0 |
| Sérialisation | 219-221 | json |

### Beat schedule (settings.py:225-277)
| Nom beat | Task | Défaut |
|---|---|---|
| `sync-firmware-build-status` | `firmware_builder.tasks.sync_firmware_build_status_periodic` | 30 s |
| `sync-device-active-status` | `dev_ctl.tasks.sync_device_active_status_periodic` | 15 s |
| `reconcile-actuator-pods` | `devices.tasks.reconcile_actuator_pods` | 300 s (k8s) |
| `poll-firmware-build-jobs` | `firmware_builder.tasks.poll_firmware_build_jobs` | 30 s (k8s_job only) |
| `cleanup-old-firmware-jobs` | `firmware_builder.tasks.cleanup_old_firmware_jobs` | 6 h (k8s_job only) |

### Tâches
**dev_ctl/tasks.py**
- `handle_sensors(user_email)` (:13-91) : heartbeat Redis `device:ping:{uid}:{dev}`, seuil 12 s, bascule `DeviceRegistry.active`. Nom legacy — tous types devices.
- `sync_device_active_status_periodic()` (:94-133) : fan-out par user.

**devices/tasks.py** (cycle de vie pods — SUPPRIMER sauf publish)
- `deploy_compute_pod` / `remove_compute_pod` (:22-79) : StatefulSets K8s. Retries 3, 60·2^n.
- `get_pod_status` / `list_all_pods` (:82-132).
- `reconcile_actuator_pods` (:135-277) : recrée/supprime pods orphelins.
- `publish_actuator_config(device_id, user_id)` (:280-323) : **GARDER** — JSON config → NATS `actuator.{uid}.{dev}.config`. Retries 3, 5·2^n.
- Helpers `build_config_json_for_device` (:326-384), `publish_to_nats` (:387-396).

**Déclencheurs** — devices/signals.py : `auto_deploy_compute_pod` (:24-70, post_save → toujours publish + deploy si 1re config enabled), `cleanup_compute_pod` (:73-102, post_delete → remove si plus aucune enabled).

**firmware_builder/tasks.py**
- `update_build_record(user_email)` (:99-115) : route argo/k8s ; **contournement bug Argo SQLite lock** : si "Failed" mais firmware en S3 (`check_firmware_exists_in_s3` via `minio.stat_object`) → Succeeded (:118-203).
- `sync_firmware_build_status_periodic` (:267-324) : tous users.
- `poll_firmware_build_jobs` (:327-394) : users avec builds incomplets seulement.
- `cleanup_old_firmware_jobs` (:397-449) : jobs > 24 h + BuildRecord réussus > 30 j.
- `delete_firmware_build_job` (:452-508) : suppression à la demande + notif WS.
- Notif WS `send_firmware_build_notification` (:63-96) sur groupe `firmware_builds_user_{id}` — uniquement sur changement de statut.

**etl/tasks.py**
- `generate_report(execution_id)` (:13-627) : fetch ES → `safe_eval` formules → agrégations → charts matplotlib → PDF WeasyPrint → upload S3 `user_{id}/reports/report_{id}.pdf` → update `ReportExecution`. `max_retries=3`, soft limit 300 s, hard 360 s, `acks_late`, `reject_on_worker_lost`. Statuts processing → completed/failed.

## 2. Commandes management NATS (consumers)

Pattern commun : `nats.connect` (5 tentatives/2 s), queue groups, handlers SIGINT/SIGTERM, shutdown graceful.

### 2.1 `consume_nats_to_elasticsearch.py`
- Topic : `sensor.*.*.measurement.>`, queue group `elasticsearch-mt-workers`.
- Valide champs unifiés (`user_id, username, pred_dev, device_id, metric_name, value, timestamp, source_type`), parse timestamp ISO/epoch, value → float.
- `UnifiedDataPoint.to_elasticsearch_doc()` : `@timestamp, pred_dev, device_id, metric_name, value, source_type` + optionnels `unit, tags, channel/state/pwm_value, formula_id/formula_name/input_devices`.
- Destination : `MultiTenantElasticsearchBatchWriter` — index par user `user_{id}_measurements`, création auto.
- Batching : par user, flush ≥ 500 docs ou ≥ 10 s ; periodic flush 30 s ; bulk `refresh=False`.

### 2.2 `consume_nats_to_redis.py`
- Topic `sensor.*.*.measurement.>`, queue `redis-workers`.
- `sensor == "ping"` → `DevicePingKey {timestamp}` ; sinon `SensorMeasurementKey {value, timestamp, user_id}`.
- Redis **db 2** (hardcodé, pas lu de `DEVICE_REDIS["DB"]` — incohérence mineure).
- TTL 86400 s. Pas de batching.

### 2.3 `etl/management/commands/etl_nat_es_consumer.py`
- Topic `etl.*.*.formula.*.result.*`, queue `etl-formula-es-workers`.
- Parse via `TopicParser.parse_etl` ; doc ES avec `formula_id/formula_type/property_index/variables/computed_at`, `message_type: "formula_result"`, `source_type: "formula"`, `pred_dev: "virtual_device"`, `metric_name: formula_name`.
- Index `user_{uid}_measurements` (même index que sensors). Batching 500/10 s.

### 2.4 `etl/management/commands/etl_nat_redis_consumer.py`
- Topic idem, queue `fluid-redis-workers`.
- Stocke `FluidPropertyValueKey` avec property_code = `{formula_type}_{property_index}`. Redis db 2 (via settings cette fois). TTL 86400 s.

### 2.5 Producteur amont — `etl/management/commands/etl_compute.py`
- Souscrit `sensor.*.*.measurement.>`, calcule formules `fluid_property` avec cache mémoire TTL 5 min, publie `etl.{uid}.{dev}.formula.{fid}.result.{idx}`.

## 3. Redis — 3 DBs

| DB | Usage | Config |
|---|---|---|
| 0 | Broker + results Celery | `CELERY_BROKER_URL` |
| 1 | Channel layer Channels (groupes WS) | `CHANNELS_REDIS_URL` |
| 2 | Device state live | `DEVICE_REDIS` |

Clés db 2 (`dev_ctl/redis_models.py`) — TTL 86400 s partout :
`DevicePingKey`, `SensorMeasurementKey`, `ActuatorStateKey`, `ControllerDesiredStateKey`, `ActuatorChannelStateKey`, `ActuatorChannelTargetKey`, `ActuatorConfigKey`, `ActuatorConnectionKey` (+`:last_seen`), `FluidPropertyStateKey/ValueKey/GroupKey` (détail dans ws-channels-crypto.md §6).

## 4. `k8s_ctl/` — SUPPRIMER (traçabilité)

- models.py/views.py vides ; l'app n'héberge que `k8s_client.py`.
- `K8sControllerManager` (:19) : StatefulSets `compute-{device_id}`, 1 replica, image `reg.pnex.io/pnex/server:latest`, commande `run_compute_controller --device-id --user-id`, env USER_EMAIL/USER_ID/DEVICE_ID, secret `pnex-server`, labels `app=compute-controller`.
- `create/delete/get_status/list/get_labels` (:59-327).

**Ce que font les pods** — `devices/management/commands/run_compute_controller.py` (355 l.) :
- Boucle 5 s : lit `ActuatorChannelConfig` ; mode **binary** → lit capteurs Redis, agrège (mean/max/min/single), publie `actuator.{uid}.{dev}.sensor_data` (la machine d'état ESP32 décide) ; **calendar** et **follow** → **TODO non implémentés** ; manual → rien.

## 5. `firmware_builder/` — flow complet d'un build

### Modèle
`BuildRecord` : `user` FK, `device_id`, `timestamp auto_now`, `success`, `argo_wf_job_name` (utilisé aussi pour K8s Jobs), `build_phase`, `firmware_bin_s3_key`. `save()` génère la clé S3 `user_{id}/firmware/{device_id}-firmware.bin`.

### Endpoints (`api/v1/`)
| Route | View | views.py |
|---|---|---|
| `build-firmware/` POST | `BuildFirmwareView` | :79-227 |
| `download/firmware/<device_id>/` GET | `FirmwareDownloadView` | :230-287 |
| `build-records/` GET | `UserBuildRecordListView` | :290-296 |
| `build-records/<pk>/` DELETE | `BuildRecordDeleteView` | :299-358 |

`BuildFirmwareView.post` : quotas par tier (:117-144), intervalle minimum entre builds (:147-160), DeviceToken + encryption key, `submit_firmware_build()`, `update_or_create` BuildRecord, notif WS initiale.

### Orchestrateur — `firmware_build_manager.py`
- Backend selon `FIRMWARE_BUILD_BACKEND` : `argowf` | `k8s_job`.
- `_submit_argo_workflow` (:219-280) : params **tous base64** (git-repo, git-ref, firmware-type, wifi-ssid, wifi-password, host, user-id, token, device-id, encryption-key, insecure, server-port, metadata, chip).
- `_submit_k8s_job` (:282-322).

### Backend k8s_job — `k8s_job_manager.py` (798 l.)
- Job `firmware-build-{device_sanitized}-{YYYYmmdd-HHMMSS}`, image `shanisma/pnex-firmware-builder:latest`, env S3 en clair, `backoff_limit=2`, `ttl_seconds_after_finished=3600`, `active_deadline_seconds=1800`, resources 500m/1Gi → 2000m/4Gi.
- **Script de build** `_build_container_args` (:633-797) — ce que la crate Rust devra reproduire :
  1. Decode base64 des params → env
  2. `git clone $GIT_REPO /workspace` + `checkout $GIT_REF`
  3. `cd $FIRMWARE_TYPE` + vérif `platformio.ini`
  4. **`pio run --verbose`** (PlatformIO)
  5. Find `.pio/build/**/firmware.bin` (fallback `*.bin`)
  6. **`python -m esptool merge-bin`** : ESP8266 → flash @0x0 ; ESP32 → bootloader @0x1000 + partitions @0x8000 + firmware @0x10000 (`--flash-mode dio --flash-freq 40m --flash-size 4MB`)
  7. AWS CLI : `aws s3 cp ... s3://$S3_BUCKET/user_$USER_ID/firmware/{device_id}-firmware.bin --endpoint-url=$S3_ENDPOINT`

### Backend argowf — `src/argowf/__init__.py`
`ArgoWfSubmitter` : URLs API, header Bearer, `set_wf`/`submit_wf`/`get_wf_status` (retry 10×/1 s, 404 → NotFound)/`wait_wf_completion`.

### Statuts & polling
`build_phase` : Pending/Running/Succeeded/Failed/Deleted (+Starting interne). Polling beat 30 s → notif WS sur changement uniquement.

## 6. `argo_wf/` — templates
- `build-firmware.yaml` (259 l.) : même script de build que k8s_job_manager ; blocs envFrom secret commentés (creds S3 en env clair côté k8s_job).
- `s3-secret.yaml` : placeholders S3.

## 7. `simulator/`
Une seule commande `sim_sensors.py` (204 l.) : simule des devices capteurs via **WebSocket** (`ws/sensor/ingest`), crée `DeviceRegistry` + `DeviceToken`, envoie 10 mesures `measurement=value` espacées de 0,3 s. Args : user-email, device-id, predefined-device-name, measurement, num-devices, min/max-value, websocket-url.

## 8. Config settings.py — synthèse infra

| Brique | Settings | Notes |
|---|---|---|
| NATS | `NATS_CONFIG` HOST/PORT (:399-407) | sans auth ; `NATS_SUBJECTS` vestigial |
| Redis | db 0/1/2 (:214-218, 280-294) | — |
| MinIO/S3 | `S3_CONFIG` (:355-374) | bucket unique `pnex`, Scaleway fr-par, chemins `user_{id}/firmware/` + `user_{id}/reports/` |
| K8s jobs | `KUBERNETES_JOBS` (:311-349) | ns `firmware-builds`, image builder, SSH secret non utilisé |
| K8s contrôleurs | `K8S_CONTROLLER_NAMESPACE` `pnex`, `CONTROLLER_IMAGE` (:392-396) | à supprimer |
| Argo | `ARGO_WF` (:297-305) | à supprimer |
| Firmware | `FIRMWARE_BUILD_BACKEND` défaut `k8s_job` ; `FIRMWARE_GIT` = `https://github.com/Pnex/iot-firmware.git` branche `main` (:245-251, 380-383) | — |
| EMQX | **absent** | 2 commentaires historiques seulement ; bascule WebSocket-only faite |
| ES | `ELASTICSEARCH_CONFIG` + `ELASTICSEARCH_CONSUMER` (batch 500 / flush 10 s / prefix `user_`) (:427-446) | — |

## Annexe — points clés pour Loco/Rust

1. **Worker Loco (queue PG)** : 5 tâches beat + on-demand (`handle_sensors`, `update_build_record`, `publish_actuator_config`, `generate_report`, `delete_firmware_build_job`). Le fan-out par user est remplaçable par un job unique itérant les users.
2. **Redis db 2** (device state) = seule vraie dépendance Redis métier → candidates : Postgres TTL logique, ou conservation Redis.
3. **Redis db 1** (channels) : notifs WS firmware → alternative Loco.
4. **firmware-builder Rust** : reproduire le script build (git clone → pio run → esptool merge-bin → upload S3) en shell-out.
5. **Suppression pods actuateurs** : k8s_ctl/, tasks deploy/remove/reconcile, signals, run_compute_controller, control_actuator. La logique binary (agrégation → `sensor_data`) part à l'edge.
6. **Argo** : supprimer src/argowf/, argo_wf/, settings, et le contournement du bug SQLite lock.
