# Devices custom (Tier 2) — firmware générique en package PIO

> Référence utilisateur pour les devices « custom » (type `mixed`) :
> compilez le firmware générique **chez vous**, sur **n'importe quelle
> carte** supportée par PlatformIO, avec vos pins déclarées dans votre
> sketch. Doc architecturale : `docs/architecture/firmware-build.md` §5.

## Le modèle en deux étages

| | Tier 1 — validé/strict | Tier 2 — on-the-fly |
|---|---|---:|
| Catalogue | presets overlay (soil_sensor, generic_esp8266/c3) | `custom_device` (type `mixed`) |
| Compilation | serveur (POST /build-firmware → .bin) | l'utilisateur (`pio run` local) |
| Cartes | overlay boards codées en dur (NodeMCU, XIAO) | toute carte PIO (8266 / ESP32 / C3 / S3…) |
| Pins | dérivées de l'overlay board | déclarées dans le sketch (`addInput`/`addOutput`/`addAnalogInput`) |
| Flash | navigateur (Web Serial) | `pio run -t upload` locale |
| Validation | chip-caps strictes (overlay + soc du board) | chip-caps strictes si SoC connu, permissive sinon (warn) |

## Démarrage rapide

1. **Créez un device** : page Devices → *Ajouter un device* → modèle
   « Custom Device (Dynamic) » → renseignez WiFi, hôte du serveur et
   votre carte.
2. **Copiez les deux snippets** générés (bouton *Copier* sur chacun) :
   - `platformio.ini` — tous les secrets préremplis (defines b64 :
     WIFI_SSID / WIFI_PASSWORD / HOST / TOKEN / DEVICE_ID /
     ENCRYPTION_KEY en base64, WS_SSL en clair — contrat
     `firmware-build.md` §2.1) ;
   - `src/main.cpp` — le sketch PNeX : vos pins déclarées, zéro secret.
3. `pio run -t upload && pio device monitor`

À la première connexion, l'announce porte vos pins : le serveur les
valide (chip-caps : flash, strapping, ADC, input-only), les persiste et
renvoie le `ProvisionAck`. Les labels du sketch deviennent les séries
OpenObserve `{org}/{device}/{label}` et les entrées de la page Pins.
La lib `PneX` s'installe seule au premier build (registre PIO).

## Le sketch — déclarer ses pins

```cpp
#include <Pnex.h>

PnexDevice pnex;

void setup() {
    pnex.addOutput(5, "relay1");       // sortie, repos = LOW
    pnex.addOutput(4, "pump", true);   // sortie, repos = HIGH (safe-state)
    pnex.addInput(12, "door", true);   // entrée numérique, pull-up interne
    pnex.addInput(13, "button");       // entrée numérique simple
    pnex.addAnalogInput(0, "photo");   // ESP8266 : gpio ignoré (A0 unique)
    pnex.begin();                       // WiFi + WS + announce (bloquant)
}

void loop() {
    pnex.loop();
}
```

- Aucune limite fonctionnelle de nombre de capteurs (garde-fou
  compile-time `PNEX_MAX_PINS=32` par défaut, surchargeable en `-D`).
- Un changement de sketch s'applique au re-announce : le sketch est la
  source des pins (ajout → ajout, retrait → purge). `interval_ms` posé
  en UI survit à un re-announce (conservé en base).
- La page Pins reste le pilotage à chaud : write (sorties), set_mode,
  subscribe (cadence) — les modifications UI sont runtime ; au
  prochain re-announce, le sketch réaffirme sa déclaration.

## Install alternatives (lib PneX)

Registre PIO (défaut généré) :

```ini
lib_deps = Pnex/PneX@^1.0.0
```

Avant publication au registre, ou hors ligne : clonez le monorepo et
pointez le dossier contenant la lib :

```ini
lib_extra_dirs =
    /chemin/vers/pnex-rust/firmware/lib
lib_deps = PneX
```

## Sécurité

- Les secrets vivent dans vos fichiers locaux (`platformio.ini`) — le
  serveur ne les stocke jamais.
- Toute perte de lien (close, PONG timeout 15 s, WiFi) → toutes les
  sorties en safe-state (choisi dans `addOutput`), puis backoff de
  reconnexion 1 s → 60 s.
- Strapping pins / flash / input-only : rejetées par les chip-caps quand
  le SoC est reconnu (esp8266, esp32-c3, esp32) ; chip inconnu →
  admission permissive (vous êtes responsable du câblage — le serveur ne
  valide pas ce qu'il ne connaît pas, warn journalisé).
- Une seule session device à la fois (anti-clone, close 4003).

## Publication (mainteneurs)

La lib vit dans `firmware/lib/pnex/` : `library.json` validé par
`pio pkg pack`, exemple inclus. Publication : `pio pkg publish
firmware/lib/pnex` (compte PIO requis, owner `Pnex` à créer) — le
snippet UI référence `Pnex/PneX@^1.0.0`. Build serveur (Tier 1) et build
utilisateur (Tier 2) partagent la même lib : une évolution de protocole
= une seule ligne à changer.
