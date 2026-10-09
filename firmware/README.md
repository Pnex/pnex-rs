# Firmware PlatformIO (workspace pnex)

Firmware ESP8266 et ESP32-C3 du projet pnex, convergé dans le monorepo —
**une version pnex (tag) = une version de firmware qui compile ensemble**
(job CI `firmware`, cf. `.github/workflows/ci.yml`).

## Layout

| Dossier | Rôle |
|---|---|
| `generic_esp8266/` | Générique pin_slave (Brick 0) — modèle générique, build serveur |
| `generic_esp32c3/` | Générique ESP32-C3 (Seeed XIAO ESP32C3) — modèle générique, build serveur |
| `lib/pnex/` | **Lib PIO `PneX`** — toute la mécanique (transport WiFi+WS+ChaCha, config -D b64, profil pin_slave, déclaration de pins sketch → `Announce.pins` pour le firmware custom). Embarquée dans le serveur et compilée avec l'IDE firmware. Exemple : `examples/CustomMetrics/` |
| `common_libs/` | Libs partagées — `lib_extra_dirs = ../common_libs` impose la structure frère : `pnex-core-cpp` (miroir de `pnex_core::control`, golden vectors Rust = C++) |
| `core-cpp-tests/` | Rejoue les golden vectors de `pnex-core-cpp` sur l'hôte (Unity) — `uv run pio test -d core-cpp-tests -e native` ; regen des vecteurs : `PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core --test golden_vectors` |

> **PneX (2026-09-14)** : `pnex-transport`, `crypto` et `config` sont
> **déplacés dans `lib/pnex/`** — la lib est autonome pour la publication
> PIO (un package publié ne peut pas référencer `../common_libs`). Les
> projets du monorepo la consomment via `lib_extra_dirs = ../lib`
> (multi-lignes obligatoire — un `lib_extra_dirs = a b` est parsé comme un
> seul chemin). Le manifeste `library.json` du registre résout ses deps
> tout seul ; les inis du monorepo gardent des `lib_deps` explicites (le
> LDF ne résout pas les deps manifeste d'une lib trouvée via storage).
>
> **F1 — transport** (edge-model.md §3/§11) : la mécanique transport
> (décodage config, WiFi, WS, framing ChaCha, bookkeeping PONG) vit dans
> `lib/pnex/src/pnex_transport.*` ; les mains gardent la policy haut
> niveau. `pnex_config.h` (ex config.h) n'est inclus que par
> `pnex_transport.cpp` — globals non const = symboles dupliqués au link
> sinon.

Le worker de build (`crates/pnex-firmware-builder`) compile les presets
avec la config device en variables d'environnement (base64) — contrat
détaillé dans `docs/architecture/firmware-build.md`. Le firmware custom
(IDE, modèles génériques uniquement) passe par le même worker — voir
`docs/architecture/custom-firmware.md`.

## Toolchain

```bash
uv sync                # venv pio/esptool épinglé par uv.lock
PNEX_PIO_BOARD=nodemcuv2 uv run pio run -d generic_esp8266
```

Le serveur embarque cette arborescence à la compilation
(`FirmwareSource::Embedded`) : le binaire déployé (Raspi, self-hosted) build
*sa* version du firmware sans clone git ni chemin local. Seule la toolchain
`pio` doit être installée sur la machine.

## Docker (image de build cloud)

```bash
task firmware:build-docker    # ← depuis la racine du monorepo
```
