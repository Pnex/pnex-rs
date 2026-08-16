# Build firmware côté serveur — contraintes du dépôt `pnex-firmwares`

> **Statut : IMPLÉMENTÉ (Phase 6, 2026-08-16).** Ce document conserve (a)
> l'architecture cible validée (Appendice X), (b) les faits **vérifiés** dans
> le dépôt firmware `/home/shan/Documents/shan-perso/pnex-firmwares` qui
> conditionnent l'interface du worker, et (c) les écarts de l'implémentation.
>
> **Réalisé dans** : crate `pnex-firmware-builder` (pipeline + `ArtifactStore`),
> worker `BuildFirmwareWorker` (queue PG loco `pg_loco_queue`, SKIP LOCKED
> intégré), `controllers/builds.rs` (contrat `docs/contracts/build.http`),
> pages front Builds + enregistrement avec build auto.
>
> **Écarts vs la conception initiale** :
> - chemins de parité Django (`/api/v1/build-firmware`, `/build-records`,
>   `/download/firmware/{device_id}`) au lieu du `POST /builds` évoqué en §1 ;
> - `ArtifactStore` à deux backends (décision user) : `local` (FS, edge)
>   implémenté d'abord, `s3` différé (erreurs `NotImplemented` claires),
>   sélection `STORAGE_BACKEND` (env) surchargeant la config — pas de
>   MinIO dans compose pour l'instant ;
> - logs : `tracing` serveur + queue des 30 dernières lignes dans l'erreur
>   du record — le stream des logs vers OpenObserve est **différé** ;
> - suivi des builds par **polling** front (~5 s, décision user) — pas de
>   WS `ws/firmware/builds` (retiré de la Phase 6, cf. inventory §4) ;
> - secrets WiFi/hôte transportés dans `pg_loco_queue.task_data` (limite
>   documentée : visibles de l'admin DB, parité spec k8s Django ; purge via
>   `cargo loco jobs clear-jobs`) ; **token + clé relus en base** par le
>   worker, jamais en queue ; workspace tmp effacé au drop (secrets
>   compilés dans les artefacts intermédiaires) ;
> - cache : `~/.platformio` partagé (préchauffer une fois par machine) ;
>   le cache proxy dédié sccache/bucket est **différé** ;
> - cancellation tokens et clés de config de rétention (D6) : structure
>   posée, gestion différée.

## 1. Architecture cible (Appendice X, résumé)

- Le build est un **job asynchrone** : `POST /builds` → enregistrement
  `Build` (status=queued) → enqueue **queue PostgreSQL** (Loco, `SKIP LOCKED`)
  → réponse immédiate `{build_id}`. Le handler HTTP **ne compile jamais**.
- Un **worker Loco** (`cargo loco start --worker`, ou `--server-and-worker` en
  self-hosted) claim le job, passe status=running, pilote la toolchain en
  **sous-process** (`tokio::process::Command`), stream les logs vers
  OpenObserve, pousse l'artefact `.bin` vers MinIO/S3, pose
  status=succeeded/failed.
- `num_workers` bas (1–2) par process ; scaling horizontal (réplicas), pas
  vertical. Timeout dur 10–15 min, retries bornés (échecs compilation
  déterministes). Cancellation tokens pour l'annulation utilisateur.
- Cache `sccache`/`ccache` + deps toolchain sur volume/bucket partagé.
- **Secrets injectés au build dans le worker** (seul tier à accès au store) ;
  jamais dans le tier API. Deux images en SaaS : API slim, worker fat
  (toolchain). Même binaire, flag de lancement différent.
- Front Dioxus CSR = assets statiques servis par Loco — pas de pod web.

## 2. Contrat de build constaté dans `pnex-firmwares` (vérifié)

Le dépôt firmware est un workspace **PlatformIO** (ESP8266, framework
Arduino) : projets `soil_sensor`, `4_chan_relay`, `tft_dev` + libs partagées
`common_libs` (config, display) + outils dev `ws-server` (Python).

### 2.1 Les build args = variables d'environnement → `-D` defines

Chaque `platformio.ini` (`soil_sensor`, `4_chan_relay`) déclare :

```ini
build_flags =
    -D WIFI_SSID=\"${sysenv.WIFI_SSID}\"
    -D WIFI_PASSWORD=\"${sysenv.WIFI_PASSWORD}\"
    -D HOST=\"${sysenv.HOST}\"
    -D TOKEN=\"${sysenv.TOKEN}\"
    -D DEVICE_ID=\"${sysenv.DEVICE_ID}\"
```

→ **le worker doit transmettre la config device en variables d'environnement
du sous-process `pio run`**, pas en argv. Valeurs consommées par
`common_libs/config/config.h` (`#ifndef` + défauts) :

| Variable | Encodage | Exemple vérifié (`4_chan_relay/build.sh`) |
|---|---|---|
| `WIFI_SSID` | clair | `Coloc` |
| `WIFI_PASSWORD` | clair | mot de passe WiFi |
| `HOST` | **base64** | `ZGV2MS5wbmV4Lmlv` = `dev1.pnex.io` |
| `TOKEN` | **base64** | token du device (cf. `device_tokens`) |
| `DEVICE_ID` | **base64** | `cHN5Y2hvbG9naWNhbC10ZQo=` = `psychological-te` |

`4_chan_relay` ajoute des flags fixes : `CORE_DEBUG_LEVEL=3`,
`PB_FIELD_16BIT=1`, `PB_ENABLE_MALLOC=1` (nanopb — proto binaire sur WS).

### 2.2 Pattern d'invocation local (`build.sh` de chaque firmware)

```bash
export WIFI_SSID=... WIFI_PASSWORD=... HOST=... TOKEN=... DEVICE_ID=...
uv run pio "$@"       # pio run | pio run --target upload | pio device monitor
```

Le worker réplique ce pattern : spawn `pio run` (dans l'image Docker
`pio-builder`) avec l'env ci-dessus, cwd = sous-dossier du firmware
(`soil_sensor/`, `4_chan_relay/`… — `lib_extra_dirs = ../common_libs` impose
la structure du workspace complet).

### 2.3 Image de build

`Dockerfile` du dépôt : `python:3.12` + pio + AWS CLI + protobuf-compiler,
**pré-build** des deps de `soil_sensor` et `4_chan_relay` (`RUN cd … && pio
run`) pour chauffer le cache d'images layers. Tag de référence :
`192.168.1.100/pnex/pio-builder:latest`.

## 3. Implications pour pnex-rust (Phase 6)

- **`BuildFirmwareArgs`** (job queue) : `build_id`, `device_id`, `target`,
  `firmware_config`. Le worker résout config + secrets (store) puis spawn
  `pio run` avec les 5 variables §2.1 (base64 pour HOST/TOKEN/DEVICE_ID —
  attention à l'encodage, pas du clair).
- **UI (page Devices / future page Builds)** — directives utilisateur :
  - le formulaire d'enregistrement devra à terme collecter **URL du serveur,
    SSID WiFi, mot de passe WiFi** (paramètres de build du firmware) ;
  - pour un **device custom** (custom_sensor/custom_device), afficher un
    **snippet de configuration** du code source pour guider l'utilisateur.
- Artefact `.bin` → MinIO/S3 (Décision D5 : parité script k8s_job Django —
  git clone → `pio run` → `esptool merge-bin` → ArtifactStore), timeout dur,
  secrets scopés org.
