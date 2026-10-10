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
> worker). Sur la machine de dev : `uv tool install platformio esptool`
> — shims en `~/.local/bin`, shebang direct vers le venv uv (pas d'appel
> `uv` à l'exécution, donc seul `~/.local/bin` doit être dans le PATH
> du process serveur ; cache `~/.platformio` conservé).
>
> **Réalisé dans** : crate `pnex-firmware-builder` (pipeline + `ArtifactStore`),
> worker `BuildFirmwareWorker` (queue PG loco `pg_loco_queue`, SKIP LOCKED
> intégré), `controllers/builds.rs` (contrat `docs/contracts/build.http`),
> pages front Builds + enregistrement avec build auto.
>
> **Écarts vs la conception initiale** :
> - chemins de parité avec la stack précédente (`/api/v1/build-firmware`, `/build-records`,
>   `/download/firmware/{device_id}`) au lieu du `POST /builds` évoqué en §1 ;
> - `ArtifactStore` — **D5 v2 (2026-08-18)** : les binaires vivent **en base**
>   (table `firmware_artifacts`, backend `db` par défaut, implémentation
>   `services/artifact_store.rs` côté backend — upsert `ON CONFLICT (key)`,
>   zéro artefact orphelin, plafond défensif 50 Mo). Backend `local` (FS)
>   **supprimé**. `s3` = tier industriel **implémenté** (Phase C, opendal —
>   cf. ci-dessous), sélection `PNEX_STORAGE_BACKEND=db|s3` (env) surchargeant
>   la config. Deux tiers de déploiement (le tier sqlite hobbyiste a été
>   retiré le 2026-10-10, décision #19) :
>   - **postgres** (scalable) : tout en PG — pods API **stateless**, n'importe
>     quel réplica sert le download (le pod worker reste stateful : toolchain
>     pio + cache `~/.platformio`, inhérent à la compilation) ;
>   - **s3** (industriel) : artefacts sur S3-compatible (AWS, RustFS,
>     Scaleway…) via opendal 0.57 — data/queue restent en PG. Config
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
>   documentée : visibles de l'admin DB, comme la spec k8s de la stack
>   précédente ; purge via
>   `cargo loco jobs clear-jobs`) ; **token + clé relus en base** par le
>   worker, jamais en queue ; workspace tmp effacé au drop (secrets
>   compilés dans les artefacts intermédiaires) ;
> - cache : `~/.platformio` partagé (préchauffer une fois par machine) ;
>   le cache proxy dédié sccache/bucket est **différé** ;
> - cancellation tokens : structure posée, gestion différée ; rétention :
>   **aucune (D6 clos)** — le binaire doit rester disponible pour
>   re-flash/recompile dans tous les cas (l'upsert par device borne le
>   volume à 1 artefact de 1–4 Mo par device).

## 1. Architecture cible (Appendice X, résumé)

- Le build est un **job asynchrone** : `POST /builds` → enregistrement
  `Build` (status=queued) → enqueue **queue loco** (PostgreSQL `SKIP LOCKED`) → réponse immédiate `{build_id}`. Le handler HTTP
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

Le firmware est un workspace **PlatformIO** (ESP8266 + ESP32-C3, framework
Arduino) : projets `generic_esp8266`, `generic_esp32c3`
(Seeed XIAO), `generic_esp32`, `generic_esp32s3`, `generic_esp32c6`
(Waveshare C6-Zero) + lib PneX `lib/pnex` (transport, crypto, config)
+ libs partagées `common_libs` (pnex-core-cpp). `soil_sensor` et sa lib
`display` sont supprimés le 2026-10-10 (capteur à refaire). Le mock Python
`ws-server` est supprimé (2026-10-09 : il parlait en clair, sans TLS).
`4_chan_relay` (nanopb, D20) supprimé le 2026-09-13.

### 2.1 Les build args = variables d'environnement → `-D` defines

Chaque `platformio.ini` (`generic_esp8266`, …) déclare :

```ini
build_flags =
    -D WIFI_SSID=\"${sysenv.WIFI_SSID}\"
    -D WIFI_PASSWORD=\"${sysenv.WIFI_PASSWORD}\"
    -D HOST=\"${sysenv.HOST}\"
    -D TOKEN=\"${sysenv.TOKEN}\"
    -D DEVICE_ID=\"${sysenv.DEVICE_ID}\"
    -D ENCRYPTION_KEY=\"${sysenv.ENCRYPTION_KEY}\"
    -D PNEX_CA_CERT=\"${sysenv.PNEX_CA_CERT}\"
    -D PNEX_CLIENT_CERT=\"${sysenv.PNEX_CLIENT_CERT}\"
    -D PNEX_CLIENT_KEY=\"${sysenv.PNEX_CLIENT_KEY}\"
```

→ **le worker doit transmettre la config device en variables d'environnement
du sous-process `pio run`**, pas en argv. Valeurs consommées par
`common_libs/config/config.h` (`#ifndef` + défauts) :

| Variable | Encodage | Exemple vérifié (build.sh historique) |
|---|---|---|
| `WIFI_SSID` | **base64** | `Q2hleiBTaGFu` = `Chez Shan` (les espaces d'un SSID littéral casseraient le flag `-D`) |
| `WIFI_PASSWORD` | **base64** | mot de passe WiFi encodé |
| `HOST` | **base64** | `ZGV2MS5wbmV4Lmlv` = `dev1.pnex.io` |
| `TOKEN` | **base64** | token du device (cf. `device_tokens`) |
| `DEVICE_ID` | **base64** | `cHN5Y2hvbG9naWNhbC10ZQo=` = `psychological-te` |
| ~~`WS_SSL`~~ | — | **supprimée le 2026-10-09** (D154) : le firmware ne parle que `wss://`/`https://`, sans `setInsecure` ; il n'existe plus de build en clair |
| `ENCRYPTION_KEY` | **base64** | `device_tokens.encryption_key` (32 octets) = clé partagée du lien Noise (D156) ; vide ou invalide → le device ne se connecte jamais (plus de mode en clair, SEC-19). Consommée par `lib/pnex/src/pnex_crypto.cpp` |
| `PNEX_FW_VERSION` | clair (id de build) | version stampée dans le binaire, annoncée à l'announce, sert de clé à l'artefact OTA versionné (2026-09-22, OTA — cf. ota.md) |
| `PNEX_CA_CERT` | **base64** (PEM) | racine épinglée WS+OTA (`pnex_tls`), **injectée automatiquement depuis D70** : le worker lit `PNEX_CA_CERT_FILE` (= `deploy/edge/pki-data/device-ca.pem` : CA locale, ou ISRG Root X1 en cloud) à chaque build ; toujours posée. **Obligatoire depuis le 2026-10-09** : vide = aucune poignée de main TLS n'aboutit (plus de `setInsecure`), le serveur refuse un tel build hors tests (`build_no_ca`) |
| `PNEX_CLIENT_CERT` / `PNEX_CLIENT_KEY` | **base64** (PEM) | identité TLS client du device (D153), émise par la CA de l'org à chaque build ; sans elle le serveur ferme en 4014 |
| `PNEX_OTA_PUBKEY` | hex (64) | clé publique Ed25519 de l'instance (SEC-18) : toute image OTA non signée par elle est refusée |
| `PNEX_OTA_ENABLE` | clair `0`/`1` | 1 sur les 4 génériques : cap `ota` à l'announce + dispatch `ota_available` |

`4_chan_relay` ajoutait des flags fixes nanopb (`PB_FIELD_16BIT=1`,
`PB_ENABLE_MALLOC=1`) — morts avec lui.

#### 2.1.1 Écran de debug local (2026-09-20)

Écran **choisi par device** (`{"screen": null|"ssd1306"|"st7735"}` dans
`device_registries.peripherals`), résolu contre le profil board v2
(`ScreenPeripheral`, builtin = soudé forcé sur `nodemcu_oled`). Le builder
pousse **toujours** les 9 vars (PIO `${sysenv.*}` échoue sur var absente) :

| Variable | Valeurs | Rôle |
|---|---|---|
| `PNEX_SCREEN_SSD1306` / `PNEX_SCREEN_ST7735` | `0`/`1` | gate du driver dans la lib PneX (`pnex_screen`) |
| `PNEX_SCREEN_SDA` `SCL` `SCK` `MOSI` `CS` `DC` `RST` | gpio décimal ou `-1` | câblage rôle→gpio du profil board |

`pnex_screen` (lib PneX) : OLED = U8g2 HW I2C, pages texte simples ;
TFT = gerbe animée (ex-démo TFT supprimée, canvas plein cadre sur
ESP32/C3/S3 ; chemin dégradé direct-draw sur ESP8266 — canvas 40 Ko inadapté).
Choix UI : picker dans l'éditeur de pinout (`BoardHeader`), builtin verrouillé.

### 2.2 Pattern d'invocation local (`build.sh` de chaque firmware)

```bash
export WIFI_SSID=$(echo -n "Chez Shan" | base64) WIFI_PASSWORD=$(echo -n <mdp> | base64)
export HOST=$(echo -n dev1.pnex.io | base64) TOKEN=$(echo -n <token> | base64) DEVICE_ID=$(echo -n <device_id> | base64)
export ENCRYPTION_KEY=<clé_noise_b64>   # device_tokens.encryption_key, déjà en base64 : telle quelle
export PNEX_CA_CERT=… PNEX_CLIENT_CERT=… PNEX_CLIENT_KEY=… PNEX_OTA_PUBKEY=…   # émis par le serveur
uv run pio "$@"       # pio run | pio run --target upload --upload-port <port> | pio device monitor
```

Le worker réplique ce pattern : spawn `pio run` (dans l'image Docker
`pio-builder`) avec l'env ci-dessus, cwd = sous-dossier du firmware
(`generic_esp8266/`… — `lib_extra_dirs = ../common_libs` impose
la structure du workspace complet).

### 2.3 Image de build

Le builder vit dans l'image `pnex-server` (`deploy/docker/Dockerfile`) :
cores PlatformIO et bibliothèques préchargés, builds hors ligne. L'ancienne
image `pio-builder` (`firmware/Dockerfile`, k8s) est supprimée
(2026-10-09).

## 3. Implications pour pnex-rust (Phase 6)

- **`BuildFirmwareArgs`** (job queue) : `build_id`, `device_id`, `target`,
  `firmware_config`. Le worker résout config + secrets (store), **extrait
  la source embarquée dans un tmp par job** puis spawn `pio run` avec les
  variables §2.1 (WIFI_SSID, WIFI_PASSWORD, HOST, TOKEN, DEVICE_ID **en
  base64** — un SSID avec espaces casserait le flag `-D` ; plus de
  `WS_SSL` : toujours wss).
- **UI (page Devices / wizard)** — livré : le wizard collecte WiFi, hôte
  (toujours wss depuis D70) **+ la carte PIO**. (Les snippets copiables du
  device custom Tier 2 sont retirés — cf. §5 ; un code utilisateur passe
  par l'IDE de firmware custom.)
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

## 5. Tier 2 — build local par l'utilisateur (package PIO, 2026-09-14)

> **Retiré de l'UI depuis l'IDE de firmware custom (D87–D94,
> `custom-firmware.md`)** : le wizard ne propose plus `custom_device` ni les
> snippets à compiler en local. Ce qui reste valable : l'admission par
> manifeste `Announce.pins` (utilisée par les firmwares de l'IDE) et la lib
> PneX commune. Familles de devices : `edge-model.md` §2 bis.
> **2026-10-02** : code Tier 2 purgé — l'admission par manifeste ne vaut
> que pour un device à `firmware_project_id`, toujours en chip-caps strictes
> (plus de mode permissif) ; la suite de cette section est historique.

Depuis la lib `firmware/lib/pnex/` (`PneX`), le firmware générique est
**publiable** (`pio pkg publish`) et compilable par l'utilisateur final :

- **Wizard custom** (`custom_device`, type `mixed`) : étape Config avec
  WiFi/hôte + sélecteur de carte (nodemcuv2 / esp32dev /
  seeed_xiao_esp32c3 / autre), puis génération de **deux snippets**
  copiables — `platformio.ini` (secrets b64 **inline**, pas de
  `${sysenv.*}` : le build utilisateur n'a pas d'env à injecter) et
  `src/main.cpp` (sketch, zéro secret). Référence utilisateur :
  `docs/DYNAMIC_DEVICES.md`.
- **Admission par manifeste** : l'`Announce` porte `pins: [PinDecl]` —
  sans overlay board, le serveur construit la pin map depuis le sketch
  (chip-caps strictes si le SoC est connu, **permissives** sinon, warn ;
  SoC observé persisté sur le device — migration 000022). Le close 4007
  « réservé aux devices génériques » est remplacé par ce chemin.
- **Presets Tier 1 inchangés** : génériques overlay
  conservent le build serveur + flash navigateur (§3/§4). Les deux
  chemins partagent la même lib PneX.

## Serveur imposé en production (`PNEX_PROD_HOST`, 2026-10-01)

Une seule variable, deux lectures :

- **Serveur (exécution)** : `PNEX_PROD_HOST=iot.example.com[:port]` (hôte
  nu ; un `https://` ou `wss://` collé est toléré). Les builds firmware
  visent toujours cet hôte, quel que soit celui envoyé par le client ; le
  référentiel « Serveurs PNeX » passe en lecture seule (`POST`/`PUT` →
  409 `edge-host-locked`) ; `GET /api/v1/edge/hosts/locked` l'expose et
  l'UI remplace le sélecteur par un affichage « imposé par ce
  déploiement » (assistant device, rebuild, page référentiels). Vide ou
  absente : comportement libre (dev, auto-hébergé).
- **Apps natives (compilation)** : la même variable exportée au moment de
  `task build:frontend:android` / desktop fige l'URL `https://<hôte>`
  dans l'app (`option_env!`, `api::config::locked_server`) : plus d'écran
  « serveur auto-hébergé », plus de bouton « changer de serveur ». Une
  valeur `http://…` complète est gardée telle quelle. Sans effet sur le
  web (même origine).
