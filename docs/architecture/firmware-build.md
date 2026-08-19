# Build firmware côté serveur — contraintes du firmware embarqué (`firmware/`)

> **Statut : IMPLÉMENTÉ (Phase 6, 2026-08-16 ; convergence monorepo
> 2026-08-18).** Ce document conserve (a) l'architecture cible validée
> (Appendice X), (b) les faits **vérifiés** dans l'arborescence
> `firmware/` du monorepo (ex-dépôt `pnex-firmwares`, aplati) qui
> conditionnent l'interface du worker, et (c) les écarts de
> l'implémentation.
>
> **Convergence monorepo (2026-08-18)** : le firmware vit dans `firmware/`
> et la source est **embarquée dans le binaire serveur**
> (`pnex_firmware_builder::embedded`, `include_dir!` — ~430 Ko). Plus de
> sélecteur de source (`FirmwareSource` Local/Git supprimé) : une version
> du serveur compile exactement la version du firmware qui l'accompagne.
> Le build extrait toujours la source dans un **tmp par job** (invariant
> SaaS/distribué : `.pio` n'écrit jamais dans la source, builds
> concurrents isolés, drop = effacement des secrets). Seule la toolchain
> `pio`/`esptool` reste externe (installée sur la machine ou l'image
> worker).
>
> **Réalisé dans** : crate `pnex-firmware-builder` (pipeline + `ArtifactStore`),
> worker `BuildFirmwareWorker` (queue PG loco `pg_loco_queue`, SKIP LOCKED
> intégré), `controllers/builds.rs` (contrat `docs/contracts/build.http`),
> pages front Builds + enregistrement avec build auto.
>
> **Écarts vs la conception initiale** :
> - chemins de parité Django (`/api/v1/build-firmware`, `/build-records`,
>   `/download/firmware/{device_id}`) au lieu du `POST /builds` évoqué en §1 ;
> - `ArtifactStore` — **D5 v2 (2026-08-18)** : les binaires vivent **en base**
>   (table `firmware_artifacts`, backend `db` par défaut, implémentation
>   `services/artifact_store.rs` côté backend — upsert `ON CONFLICT (key)`,
>   zéro artefact orphelin, plafond défensif 50 Mo). Backend `local` (FS)
>   **supprimé**. `s3` = tier industriel **implémenté** (Phase C, opendal —
>   cf. ci-dessous), sélection `STORAGE_BACKEND=db|s3` (env) surchargeant
>   la config. Trois tiers de déploiement :
>   - **sqlite** (hobbyiste) : tout (données + artefacts + queue loco
>     `sqlt_loco_queue`) dans un seul fichier — `DATABASE_URL=sqlite://…?mode=rwc`,
>     bascule one-knob (le `queue.kind` des yaml suit le schéma de l'URI via
>     Tera). Mono-pod uniquement, jamais `sqlite::memory:` (pools db et queue
>     distincts) ;
>   - **postgres** (scalable) : tout en PG — pods API **stateless**, n'importe
>     quel réplica sert le download (le pod worker reste stateful : toolchain
>     pio + cache `~/.platformio`, inhérent à la compilation) ;
>   - **s3** (industriel) : artefacts sur S3-compatible (AWS, RustFS,
>     Scaleway…) via opendal 0.57 — data/queue restent en PG ou sqlite. Config
>     `PNEX_S3_{ENDPOINT,BUCKET,REGION,ACCESS_KEY,SECRET_KEY,PATH_STYLE}` ;
>     path-style = défaut (RustFS/auto-hébergé ; `PATH_STYLE=false` = host virtuel
>     AWS), région défaut `us-east-1`, validation à la construction (config
>     incomplète → erreur explicite). Stack dev : service `rustfs` dans
>     compose.yaml (buckets `pnex`/`pnex-test` auto-créés). **⚠ Aucun
>     système de migration/réconciliation entre tiers** : on choisit à
>     l'installation, changer en cours de route = table rase ou
>     export-import manuel.
> - logs : `tracing` serveur + queue des 30 dernières lignes dans l'erreur
>   du record — le stream des logs vers OpenObserve est **différé** ;
> - suivi des builds par **polling** front (~5 s, décision user) — pas de
>   WS `ws/firmware/builds` (retiré de la Phase 6, cf. inventory §4) ;
> - secrets WiFi/hôte transportés dans le `task_data` de la queue loco
>   (`pg_loco_queue` / `sqlt_loco_queue` selon le tier — limite
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
  `Build` (status=queued) → enqueue **queue loco** (PostgreSQL `SKIP LOCKED`,
  ou sqlite selon le tier) → réponse immédiate `{build_id}`. Le handler HTTP
  **ne compile jamais**.
- Un **worker Loco** (`cargo loco start --worker`, ou `--server-and-worker` en
  self-hosted) claim le job, passe status=running, pilote la toolchain en
  **sous-process** (`tokio::process::Command`), stream les logs vers
  OpenObserve, dépose l'artefact `.bin` dans l'`ArtifactStore` (D5 v2 :
  backend `db` par défaut — table `firmware_artifacts` ; `s3` = tier
  industriel via opendal), pose status=succeeded/failed.
- `num_workers` bas (1–2) par process ; scaling horizontal (réplicas), pas
  vertical. Timeout dur 10–15 min, retries bornés (échecs compilation
  déterministes). Cancellation tokens pour l'annulation utilisateur.
- Cache `sccache`/`ccache` + deps toolchain sur volume/bucket partagé.
- **Secrets injectés au build dans le worker** (seul tier à accès au store) ;
  jamais dans le tier API. Deux images en SaaS : API slim, worker fat
  (toolchain). Même binaire, flag de lancement différent.
- Front Dioxus CSR = assets statiques servis par Loco — pas de pod web.

## 2. Contrat de build constaté dans `firmware/` (vérifié)

Le firmware est un workspace **PlatformIO** (ESP8266, framework
Arduino) : projets `soil_sensor`, `4_chan_relay`, `tft_dev` + libs partagées
`common_libs` (config, crypto, display) + outils dev `ws-server` (Python).

### 2.1 Les build args = variables d'environnement → `-D` defines

Chaque `platformio.ini` (`soil_sensor`, `4_chan_relay`) déclare :

```ini
build_flags =
    -D WIFI_SSID=\"${sysenv.WIFI_SSID}\"
    -D WIFI_PASSWORD=\"${sysenv.WIFI_PASSWORD}\"
    -D HOST=\"${sysenv.HOST}\"
    -D TOKEN=\"${sysenv.TOKEN}\"
    -D DEVICE_ID=\"${sysenv.DEVICE_ID}\"
    -D ENCRYPTION_KEY=\"${sysenv.ENCRYPTION_KEY}\"
    -D WS_SSL=\"${sysenv.WS_SSL}\"
```

→ **le worker doit transmettre la config device en variables d'environnement
du sous-process `pio run`**, pas en argv. Valeurs consommées par
`common_libs/config/config.h` (`#ifndef` + défauts) :

| Variable | Encodage | Exemple vérifié (`4_chan_relay/build.sh`) |
|---|---|---|
| `WIFI_SSID` | **base64** | `Q2hleiBTaGFu` = `Chez Shan` (les espaces d'un SSID littéral casseraient le flag `-D`) |
| `WIFI_PASSWORD` | **base64** | mot de passe WiFi encodé |
| `HOST` | **base64** | `ZGV2MS5wbmV4Lmlv` = `dev1.pnex.io` |
| `TOKEN` | **base64** | token du device (cf. `device_tokens`) |
| `DEVICE_ID` | **base64** | `cHN5Y2hvbG9naWNhbC10ZQo=` = `psychological-te` |
| `WS_SSL` | clair `true`/`false` | `true` → `wss://` (TLS, industriel), `false` → `ws://` (local/Raspberry Pi sans TLS) |
| `ENCRYPTION_KEY` | **base64** | `device_tokens.encryption_key` (32 octets ChaCha20) ; vide → frames en clair (mock `ws-server/` uniquement — le serveur réel répond `ERROR:decryption_failed` à tout et le device ne passe jamais actif). Consommée par `common_libs/crypto` |

`4_chan_relay` ajoute des flags fixes : `CORE_DEBUG_LEVEL=3`,
`PB_FIELD_16BIT=1`, `PB_ENABLE_MALLOC=1` (nanopb — proto binaire sur WS).

### 2.2 Pattern d'invocation local (`build.sh` de chaque firmware)

```bash
export WIFI_SSID=$(echo -n "Chez Shan" | base64) WIFI_PASSWORD=$(echo -n <mdp> | base64)
export HOST=$(echo -n dev1.pnex.io | base64) TOKEN=$(echo -n <token> | base64) DEVICE_ID=$(echo -n <device_id> | base64)
export ENCRYPTION_KEY=<clé_chacha20_b64>   # device_tokens.encryption_key, déjà en base64 : telle quelle
export WS_SSL=false   # true → wss (TLS), false → ws (local)
uv run pio "$@"       # pio run | pio run --target upload | pio device monitor
```

Le worker réplique ce pattern : spawn `pio run` (dans l'image Docker
`pio-builder`) avec l'env ci-dessus, cwd = sous-dossier du firmware
(`soil_sensor/`, `4_chan_relay/`… — `lib_extra_dirs = ../common_libs` impose
la structure du workspace complet).

### 2.3 Image de build

`firmware/Dockerfile` : `python:3.12` + pio + AWS CLI + protobuf-compiler,
**pré-build** des deps de `soil_sensor` et `4_chan_relay` (`RUN cd … && pio
run`) pour chauffer le cache d'images layers. Tag de référence :
`192.168.1.100/pnex/pio-builder:latest` (build via `task
firmware:build-docker`).

## 3. Implications pour pnex-rust (Phase 6)

- **`BuildFirmwareArgs`** (job queue) : `build_id`, `device_id`, `target`,
  `firmware_config`. Le worker résout config + secrets (store), **extrait
  la source embarquée dans un tmp par job** puis spawn `pio run` avec les
  variables §2.1 (WIFI_SSID, WIFI_PASSWORD, HOST, TOKEN, DEVICE_ID **en
  base64** — un SSID avec espaces casserait le flag `-D` ; `WS_SSL`
  true/false pour le schéma ws/wss du firmware).
- **UI (page Devices / future page Builds)** — directives utilisateur :
  - le formulaire d'enregistrement devra à terme collecter **URL du serveur,
    SSID WiFi, mot de passe WiFi** (paramètres de build du firmware) ;
  - pour un **device custom** (custom_sensor/custom_device), afficher un
    **snippet de configuration** du code source pour guider l'utilisateur.
- Artefact `.bin` → `ArtifactStore` (D5 v2 : extraction de la source
  embarquée → `pio run` → `esptool merge-bin` → backend `db` par défaut,
  `s3` via opendal pour le tier industriel), timeout
  dur, secrets scopés org. Le workflow CI `firmware`
  (`.github/workflows/firmware.yml`) compile les projets predefined à
  chaque changement de `firmware/` — « une version pnex = un firmware qui
  compile ».

## 4. Flash navigateur (esptool-js, Web Serial)

Le front Dioxus flashe le firmware directement depuis le navigateur via
**Web Serial** (Chrome/Edge/Opera uniquement — Firefox/Safari non supportés,
le modal affiche l'avertissement et renvoie vers le téléchargement + esptool).

- **Glue JS** : `crates/pnex-frontend/js/flasher.js` importe `esptool-js`
  (épinglé 0.6.1, npm) et expose deux globales — `pnexFlashSupported()` et
  `pnexFlash(bytes, onEvent)`. Bundlé par esbuild en IIFE
  (`npm run js:build` → `assets/flasher.js`, gitignoré, task `js:ensure`
  pour les fresh clones — même pattern que le CSS Tailwind). Chargé comme
  script classique via `asset!()` dans `App` ; consommé par
  `crates/pnex-frontend/src/flash.rs` (js-sys/wasm-bindgen, callbacks JSON).
- **Un seul `writeFlash` à l'adresse 0x0** : l'artefact servi par
  `GET /api/v1/download/firmware/{id}` est toujours l'image mergée complète
  (§1/`merge.rs` — esp8266 : image unique ; esp32 : bootloader+partitions+app
  mergées). Paramètres alignés sur le merge serveur : `dio` / `40m` / `4MB`,
  `eraseAll: false`, compression activée, baud 921600 après sync.
- **Contraintes Web Serial** : HTTPS (ou localhost), `requestPort()` doit
  partir d'un geste utilisateur — c'est pourquoi le `FlashModal` télécharge
  les octets à l'ouverture et déclenche tout le flow au clic, sans étape
  réseau intermédiaire.
- **UI** : bouton « Flasher » sur la ligne device (colonne Firmware) et sur
  l'écran de succès du wizard ; progression par étapes (connexion, écriture %,
  redémarrage) + chip détecté ; erreurs JS (annulation du sélecteur de port,
  port occupé, sync échoué) affichées en clair avec bouton « Réessayer ».
