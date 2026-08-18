# Firmware PlatformIO (workspace pnex)

Firmware ESP8266 du projet pnex, convergé dans le monorepo — **une version
pnex (tag) = une version de firmware qui compile ensemble** (job CI
`firmware`, cf. `.github/workflows/ci.yml`).

## Layout

| Dossier | Rôle |
|---|---|
| `soil_sensor/` | Capteur d'humidité du sol (predefined device) |
| `4_chan_relay/` | Actionneur 4 relais (machine à états, nanopb) |
| `tft_dev/` | Projets dev écran TFT (hors catalogue devices) |
| `common_libs/` | Libs partagées (`config`, `crypto` ChaCha20, `display`) — `lib_extra_dirs = ../common_libs` impose la structure frère |
| `ws-server/` | Mock Python du serveur WS (tests locaux de firmware) |

Le worker de build (`crates/pnex-firmware-builder`) compile ces projets avec
la config device en variables d'environnement (base64) — contrat détaillé
dans `docs/architecture/firmware-build.md`.

## Toolchain

```bash
uv sync                # venv pio/esptool épinglé par uv.lock
uv run pio run -d soil_sensor
```

Le serveur embarque cette arborescence à la compilation
(`FirmwareSource::Embedded`) : le binaire déployé (Raspi, self-hosted) build
*sa* version du firmware sans clone git ni chemin local. Seule la toolchain
`pio` doit être installée sur la machine.

## Docker (image de build cloud)

```bash
task firmware:build-docker    # ← depuis la racine du monorepo
```
