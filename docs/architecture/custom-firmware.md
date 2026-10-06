# PRD — Firmware custom : IDE intégré, source versionnée en base (D87–D94)

**Statut :** Livré, désactivé par défaut — L1, L3, L4, L5 livrés ; L2 partiel (sandbox bwrap optionnelle) ; offline livré le 2026-10-05 (§8) ; commandes custom pilotables par Device (write) (D146, 2026-10-06)
**Portée :** firmware ESP (lib PneX), build serveur, front (éditeur), protocole device
**Renvois :** `firmware-build.md` (pipeline de build par device), `edge-model.md`
(D44–D48 : manifeste de capacités, golden vectors), `ota.md`, `worker-fabric.md`
(isolation des workers), `editor-shell.md` (coquille d'éditeur), `features.md`

---

## 1. Contexte & problème

Aujourd'hui un device PneX tourne l'un de deux firmwares :

- **Tier 1 — générique preset** (`firmware/generic_*`) : `main.cpp` vide
  (`pnex.begin()` / `pnex.loop()`), 100 % piloté par le serveur via l'overlay
  board. Compilé **par device** côté serveur (secrets en `-D` défines).
- **Tier 2 — sketch custom** (ex-`firmware/lib/pnex/examples/CustomDevice`,
  supprimé le 2026-10-02) :
  l'utilisateur déclare ses pins dans le sketch (`addInput`, `addOutput`…),
  le serveur les admet via `Announce.pins`. Mais ce sketch vit **hors de
  PneX** : l'utilisateur doit installer PlatformIO, récupérer la lib PneX
  (publiée sur le registre PIO), compiler et flasher lui-même.

> **Mise à jour 2026-10-01** : le Tier 2 local est retiré de l'assistant ;
> l'IDE le remplace. Les trois familles de devices (générique PneX, IDE
> custom, cartes prêtes à l'emploi) sont posées dans `edge-model.md` §2 bis.

Conséquences :

1. Tout besoin « un peu custom » (lire un capteur I2C, calculer, publier une
   valeur) sort du produit — alors que la règle produit est que **l'UI est
   la seule interface utilisateur**.
2. On doit maintenir et publier la lib PneX sur le registre PIO, avec un
   risque de désynchronisation version serveur ↔ version lib.
3. Pas de traçabilité : on ne sait pas quel code tourne sur quel device.

## 2. Proposition

Un **IDE firmware dans le navigateur**, sur le modèle de l'éditeur de
fonctions (flow → fonction) :

- l'utilisateur écrit **un seul `main.cpp`** (V1) qui inclut `<Pnex.h>` ;
- il dispose de **toute la lib PneX** (transport chiffré, OTA, écran debug,
  caméra…) dans la version exacte du serveur ;
- la **board** vient de la variante figée du device (aucune nouveauté) ;
- il choisit des **libs additionnelles** dans un catalogue épinglé ;
- le code est **versionné en base** (Postgres, ou sqlite pour le tier
  hobbyiste), chaque build pointe une révision immuable ;
- compilation, diagnostics par ligne, flash USB ou OTA — tout depuis l'UI.

Estimation de couverture : un `main.cpp` + un catalogue de libs courantes
couvre l'écrasante majorité des cas custom (capteur I2C/SPI/1-Wire, un peu
de logique locale, publication de valeurs, commandes depuis un flow).

### 2.1 Ce qui est réutilisé tel quel

| Brique | Existant | Changement |
|---|---|---|
| Build par device | `pnex-firmware-builder` + `BuildFirmwareWorker`, tmp par job, secrets en env de `pio run` | La source du `src/main.cpp` devient une révision en base au lieu de la source embarquée |
| Lib PneX | `firmware/lib/pnex`, embarquée (`include_dir!`), `lib_extra_dirs` | Aucun — fournie par le serveur |
| Board | Variante figée au register (`PNEX_PIO_BOARD`, `PNEX_BOARD_NAME`) | Aucun |
| Admission des pins | `Announce.pins` validé par les chip-caps | **2026-10-02** : un device à `firmware_project_id` est admis par ses pins déclarées, jamais par l'overlay (zéro pin déclarée = pin map vide) ; avant, l'overlay du modèle générique reprenait la main à chaque announce et remettait tous les pins en `digital_in`, I2C compris |
| Versioning binaire / OTA | version = id du build record ; `firmware_artifacts` | Le build record référence la révision source |
| Éditeur | `code_highlight.rs` (overlay textarea) + `code_editing.rs` | Ajout du langage C++ (V1) |

### 2.2 Ce qui manque (le vrai travail)

1. **API lib de métriques et commandes custom** (§4) — prérequis n°1.
2. **Modèle de données source / révisions** (§5).
3. **Build depuis une révision + diagnostics structurés** (§6).
4. **Isolation du worker de build** (§7) — non négociable dès la V1.
5. **Catalogue de libs épinglées, préchargées pour l'offline** (§8).
6. **Page IDE** dans l'EditorShell (§9).

## 3. Non-objectifs (V1)

- Multi-fichiers (`.h`/`.cpp` additionnels), `platformio.ini` éditable.
- Libs arbitraires du registre PIO ou uploadées (V2+, §8.3).
- Autocomplétion sémantique (clangd) — V2 (§9.2).
- Frameworks autres qu'Arduino (ESP-IDF pur).
- Debug pas-à-pas (JTAG) ; moniteur série dans le navigateur (hors sujet,
  à part le pont WebSerial existant pour le flash).

## 4. Lib PneX : métriques et commandes custom

### 4.1 Constat

`Pnex.h` ne sait publier que des **pins** (`StateReport` par gpio).
`sendJson` existe mais est brut (plafond 384 octets, hors contrat). Le cas
d'usage « lire une valeur I2C puis la publier » n'a pas de chemin propre.

### 4.2 API proposée

```cpp
#include <Pnex.h>
#include <Adafruit_BME280.h>

PnexDevice pnex;
Adafruit_BME280 bme;

void setup() {
    Wire.begin(21, 22);
    bme.begin(0x76);

    // Announced in the manifest; becomes the O2 series {org}/{device}/temp.
    pnex.addMetric("temp", "°C");
    pnex.addMetric("humidity", "%");

    // Command callable from a flow (device-write / RPC), answered with Ack.
    pnex.onCommand("calibrate", [](JsonVariantConst args) {
        return true;  // ok → Ack{ok:true}
    });

    pnex.begin();
}

unsigned long last = 0;

void loop() {
    pnex.loop();  // mandatory on every iteration (WS ping, OTA, reads)
    if (millis() - last >= 10000) {
        last = millis();
        pnex.publish("temp", bme.readTemperature());
        pnex.publish("humidity", bme.readHumidity());
    }
}
```

- `addMetric(id, unit)` : entrée du manifeste de capacités (D47), famille
  `metric`, avant `begin()` (l'announce la porte). Garde-fou compile-time
  `PNEX_MAX_METRICS` (défaut 32, comme `PNEX_MAX_PINS`).
- `publish(id, value)` : surcharges `float`, `int32_t`, `bool`,
  `const char*` (courte). Identifiant inconnu = ignoré + log série.
- `onCommand(name, cb)` : la commande est annoncée (manifeste), le serveur
  n'envoie que des commandes connues ; le retour du callback produit l'`Ack`.

### D87 — Métrique custom = capacité D47 + `StateReport` par `cap_id`

Le protocole a déjà le routage sémantique : `StateReport.cap_id` → série O2
`{org}/{device}/{capacité}` (D47). Plutôt qu'un nouveau message, on
**réutilise ce chemin** : une métrique est une capacité de famille `metric`,
publiée en `StateReport { cap_id, value, … }`.

Seul changement de fil : `gpio` devient optionnel (`Option<u16>`,
additif — un fil historique porte toujours `gpio`). Alternative écartée :
`DeviceMsg::Metric` dédié (deux chemins d'ingestion pour la même chose).
Golden vectors (F3) étendus au cas sans `gpio`.

> ⚠ Rappel `arbitrary_precision` : `DeviceMsg` est une enum taggée interne —
> tout ajout de champ se valide en `cargo test --workspace`, pas en `-p`.

### D88 — Commande custom = `ServerMsg::Command { cmd_id, name, args }`

Nouveau message serveur additif ; le device répond par l'`Ack` existant.
Côté flow : tranché par D146 — c'est le nœud **Device (write)** qui cible
une commande du manifeste (pas de nœud `device-command`). Un firmware
qui ne la connaît pas répond `Ack{ok:false, err:"unknown_command"}`.

### D146 — Device (write) envoie aussi les commandes du firmware (2026-10-06)

Choix utilisateur, par symétrie avec Device (read) qui lit déjà les
métriques custom : **deux nœuds device seulement**, lecture et écriture.

- `DeviceWriteConfig.commands` (défaut vide, rétro-compatible) : commandes
  cochées dans l'inspecteur parmi celles du dernier announce (réponse
  `pinout`, champ `commands`). Ancres d'entrée = pins puis commandes, noms
  uniques (`device_duplicate_pin`), nom valide (`device_bad_command`) ;
  `pins` peut être vide si une commande est cochée.
- Routage identique aux pins : clé d'une map, ou `msg.topic` posé par le
  tagger du fil tiré sur l'ancre. La route interne
  `/internal/flow/device-write` reçoit `commands: {nom: valeur}` et pousse
  `ServerMsg::Command { name, args: {"value": valeur} }`, **uniquement**
  pour une commande annoncée (sinon résultat `ok:false`, rien n'est
  poussé) ; mêmes gardes jeton / fencing / `offline` que les pins.
- Une commande n'est pas un pin : hors règle « une source d'écriture par
  sortie » (D128), pas de `reserved_by`.
- « Créer le flow » (surfaces) : un switch / slider / bouton / nombre
  propose pins de sortie **et** commandes ; une couleur, une option ou une
  consigne ne propose **que** des commandes (un pin refuserait 0xRRGGBB).
- Correctif livré avec : la projection rembourre les 2 sorties du Device
  (write) (passthrough + nom du device) — sans fil sur la sortie « nom »,
  le message était rejeté et la sortie 0 restait muette.

### 4.3 Contraintes pédagogiques (squelette + doc)

- `pnex.loop()` à **chaque** itération ; aucun `delay()` long : au-delà du
  timeout PING, le lien tombe et les sorties passent en safe-state.
- Le squelette généré montre le motif `millis()` non bloquant.
- Les secrets (WiFi, token, clé) ne sont **jamais** dans le `main.cpp` :
  ils restent des défines injectés par le builder.

## 5. Modèle de données

### D89 — Source firmware versionnée en base, révisions immuables

```
firmware_projects
  id (uuid), org_id, name, description,
  chip_family,            -- esp32 | esp32c3 | esp32s3 | esp8266 (garde de compat)
  lib_deps (json),        -- entrées du catalogue épinglées [{id, version}]
  head_revision_id,       -- pointeur « dernière révision »
  created_at, updated_at

firmware_revisions      -- append-only
  id (uuid), project_id, parent_id (nullable),
  main_cpp (text),        -- plafond défensif (ex. 256 Ko)
  lib_deps (json),        -- figées à la révision
  content_hash (sha256),  -- dédup : même contenu = pas de nouvelle révision
  author_id, message, created_at
```

- **Rattachement device** : un device référence un projet
  (`devices.firmware_project_id`, nullable — null = générique preset). La
  board reste celle du device ; le projet déclare seulement une famille de
  chip compatible (garde au build).
- **Build record** : `build_records.firmware_revision_id` (nullable = build
  générique). On sait donc toujours quel code tourne où, et l'OTA d'une
  révision antérieure = rollback.
- Sauvegarde = nouvelle révision si le hash change (auto-save brouillon
  côté front, révision explicite au « Enregistrer » ou au build).
- Suppression d'un projet : refusée (409) tant qu'un device y est rattaché.
- Portabilité sqlite ↔ PG : types simples (text/json/uuid), pas de
  fonctionnalité PG-only (cf. piège migration 000029).

## 6. Build depuis une révision

### 6.1 Pipeline

Identique au générique (`firmware-build.md`), avec deux différences :

1. Le workspace tmp reçoit un projet PIO **généré par le serveur** :
   `platformio.ini` = gabarit par famille de chip (défines, board, lib PneX)
   + `lib_deps` résolus depuis le catalogue ; `src/main.cpp` = contenu de la
   révision. L'utilisateur n'édite **jamais** le `platformio.ini` (c'est
   lui qui porte les secrets et la politique de libs).
2. Le build record garde la **sortie complète** du compilateur (plafonnée)
   et une liste de **diagnostics structurés**.

### 6.2 Deux modes

- **Vérifier** : compile sans secrets réels (défines factices), sans
  artefact conservé, avec un cache `.pio/build` par projet (pas de secret
  dedans) → retour en quelques secondes après le premier build.
- **Compiler pour le device** : chemin actuel (secrets, artefact en base,
  flash USB ou OTA).

### D90 — Diagnostics gcc structurés, mappés sur le `main.cpp`

Parser `file:line:col: (error|warning|note): message` ; seules les lignes
pointant `src/main.cpp` sont projetées dans l'éditeur, les autres
(lib, catalogue) sont affichées en liste avec le fichier d'origine.
Le message gcc reste verbatim (exception i18n « diagnostics runtime »).

## 7. Sécurité du build

Compiler du C++ utilisateur = **exécuter du code utilisateur sur le
worker**, même avec un seul fichier :

- `#include "/proc/self/environ"` ou `#include "/chemin/config.yaml"` fait
  entrer des fichiers du worker dans le binaire — que l'utilisateur
  télécharge ensuite ;
- une lib avec `library.json` → `extraScript` exécute du Python arbitraire
  (RCE directe) ; même risque avec `extra_scripts` dans `platformio.ini`
  (d'où son non-éditabilité, §6.1).

### D91 — Worker de build firmware isolé dès la V1

- Processus `pio run` **sans secret serveur** dans son environnement (seuls
  les défines du device ciblé) ; jamais de `DATABASE_URL`, clés S3, etc.
- **Réseau coupé** pendant la compilation (`PLATFORMIO_OFFLINE`-like +
  isolation réseau du conteneur/namespace) : les libs viennent du cache.
- FS en lecture seule hors du workspace du job ; utilisateur non privilégié.
- Garde lexicale en complément (pas en remplacement) : refus des
  `#include` à chemin absolu ou remontant (`..`) dans `main.cpp`.
- Limites de ressources : timeout, CPU, mémoire, taille de sortie.

Mise en œuvre : sandbox légère sur le worker co-localisé (bubblewrap /
namespaces) en V1 ; dans la fabric (`worker-fabric.md`), un profil de
worker « firmware-build » en conteneur dédié. Tant que D91 n'est pas en
place, la feature reste désactivée par défaut (flag admin plateforme).

## 8. Catalogue de libs et offline

### D92 — Catalogue curé, versions épinglées, préchargé dans l'image worker

- Un fichier versionné dans le repo (`firmware/lib_catalog.toml`) :
  `id`, `pio_spec` (`owner/name@=x.y.z`), familles de chip compatibles,
  description, exemple minimal.
- Premier lot : BME280/BMP280, SHT3x, AHT20, BH1750, INA219, ADS1115,
  OneWire + DallasTemperature, DHT, Adafruit NeoPixel, ESP32Servo, plus
  les libs déjà tirées par PneX (U8g2, GFX, ST7735). Exclus
  volontairement : les clients de transport (MQTT, HTTP cloud…) — le
  transport, c'est PneX.
- Préchargement (livré 2026-10-05, 0.1.0-beta.4) : au build de l'image
  builder, `deploy/docker/prewarm/seed_libdeps.sh` installe, sans compiler,
  les `lib_deps` de chaque projet compilé par le worker **plus** les entrées
  du catalogue compatibles avec son SoC (`deploy/docker/prewarm/lib_catalog.txt`,
  généré depuis `LIB_CATALOG`, test `prewarm_lib_list_matches_the_catalog`)
  dans `<core>/pnex-libdeps/<projet>`. Le worker **copie** ce semis dans le
  `.pio/libdeps` de chaque build (jamais un dossier partagé : un build
  n'écrit pas là où lit celui d'une autre org). ~380 Mo.
- Deux cores PlatformIO : pioarduino (core Arduino 3.x, C6) dans
  `PNEX_PIO_CORE_DIR_PIOARDUINO`, séparé du core officiel — les deux
  plateformes installent `framework-arduinoespressif32` / `tool-esptoolpy`
  sous le même nom et s'écrasaient (re-téléchargement à chaque bascule).
  Le worker choisit le core d'après la `platform` de l'ini.
- Résultat : les 7 projets du worker et les builds custom avec libs du
  catalogue compilent en `--network none` (sites isolés).
- Le serveur **refuse** tout `lib_deps` hors catalogue (400, code machine).
- Le catalogue est exposé en API (`GET /api/v1/firmware/lib-catalog`) pour
  le picker de l'IDE.

### 8.3 Plus tard (à garder en tête, hors V1)

- **Upload de lib** (zip) stockée en base, scannée (pas de `extraScript`,
  pas de `library.json` exécutable), par org.
- **Mode « registre ouvert »** activable par l'admin plateforme (`/system`)
  pour les installs qui acceptent le risque : `lib_deps` libres, réseau
  autorisé pendant la résolution, toujours dans la sandbox D91.
- **Mode strict** pour les personnes sensibles à la sécurité : catalogue
  restreint par org, voire vide (lib PneX seule).

## 9. IDE (front)

### 9.1 V1

Nouvelle page `/firmware` (liste des projets) et `/firmware/{id}`
(éditeur), dans l'EditorShell :

- éditeur : `CodeEditor` existant + tokenizer C++ (mots-clés, préproc,
  littéraux, commentaires) + indentation 4 espaces ;
- squelette de nouveau projet = variante du `CustomDevice` avec
  `addMetric` / `publish` / motif `millis()` ;
- panneau latéral : libs du catalogue (ajout/retrait), board et devices
  rattachés, **Référence API PneX** (même principe que la référence des
  fonctions) ;
- barre d'actions : Enregistrer (révision + message), Vérifier, Compiler
  pour un device → flash USB (pont existant) ou OTA ;
- diagnostics D90 soulignés dans la gouttière + liste cliquable ;
- historique des révisions avec diff et « restaurer ».

Toute chaîne visible passe par `t!` (fr-FR + en-US) ; erreurs serveur en
codes machine enregistrés dans `err_codes::ALL`.

### D93 — Éditeur : overlay maison en V1, CodeMirror 6 en V2

L'overlay textarea suffit pour un fichier de quelques centaines de lignes
et garde zéro dépendance. En V2, CodeMirror 6 (bundlé esbuild comme
maplibre/pannellum, avec attention au poids du bundle) apporte
autocomplétion et navigation ; Monaco est écarté (trop lourd, workers).

### 9.2 V2 — autocomplétion sémantique

clangd côté worker, exposé en LSP sur WebSocket, avec le
`compile_commands.json` du projet généré (même sandbox D91). Coûteux :
seulement si la V1 montre un usage réel.

## 10. Rattachement au device et cycle de vie

### D94 — Un device suit une révision déployée, pas la tête du projet

- Rattacher un device à un projet ne reflashe rien.
- « Déployer » = build de la révision choisie pour ce device → OTA/flash.
- L'UI du device affiche : projet, révision déployée, révision de tête
  (badge « mise à jour disponible » si différent).
- Un firmware custom qui plante en boucle ne doit pas rendre le device
  irrécupérable : rollback OTA vers la révision précédente, et au pire
  flash USB. Le rollback automatique (partition précédente si pas
  d'announce après N s) est une suite souhaitable, liée à `ota.md`.

## 11. Découpage

| Lot | Contenu | Dépend de |
|---|---|---|
| L1 — Lib | `addMetric` / `publish` / `onCommand`, D87/D88 côté serveur (admission, ingestion O2, flow), golden vectors | — |
| L2 — Sandbox | D91 sur le worker actuel, flag admin plateforme | — |
| L3 — Données | Migrations D89, CRUD projets/révisions, catalogue D92 + préchargement | — |
| L4 — Build | Gabarits `platformio.ini` par famille, build depuis révision, mode Vérifier, diagnostics D90 | L2, L3 |
| L5 — IDE | Pages `/firmware`, tokenizer C++, référence API, révisions/diff, deploy | L1, L4 |
| L6 — E2E | Banc matériel : BME280 I2C sur ESP32 + ESP8266, publish → O2 → flow → notification | L5 |

L1 est utile seul (les sketchs Tier 2 hors PneX en profitent) ; L2 est un
gain de sécurité même sans l'IDE.

## 12. Conséquences

- La lib PneX n'a plus besoin d'être publiée sur le registre PIO : le
  serveur est la source de vérité de sa version. La publication peut rester
  optionnelle pour les usages hors PneX.
- Le « générique preset » reste le chemin par défaut (zéro code) ; le
  projet custom est une option par device.

## 13. Décisions à trancher

1. ~~Nœud flow : étendre device-write ou nœud dédié (D88)~~ → étendre
   Device (write), D146.
2. Périmètre exact du premier lot du catalogue (D92).
3. Technique de sandbox V1 sur le worker co-localisé (bubblewrap vs
   conteneur éphémère) — à aligner avec la fabric (P1.6).
4. Plafond de taille du `main.cpp` et du nombre de révisions conservées
   (tout garder vs purge des révisions jamais buildées).

## 14. État d'implémentation (2026-09-29)

**Livré et testé**

- **L1 — lib + protocole** : `addMetric` / `publish` / `onCommand` dans la lib
  PneX (`PNEX_MAX_METRICS` 16, `PNEX_MAX_COMMANDS` 8), caps `metric` /
  `command` dans l'announce ; `StateReport.gpio` optionnel (D87),
  `ServerMsg::Command` (D88). Serveur : manifeste persisté
  (`device_registries.announced_caps`), métriques ingérées en
  `source_type=custom_metric` (ids annoncés seulement — une métrique non
  annoncée est jetée), `POST /devices/{id}/custom-commands`. Exemple
  `CustomMetrics` = squelette de l'IDE (compile ESP32 + ESP8266).
- **L3 — données** : migration 000038 (tables `firmware_projects` /
  `firmware_revisions`, ids `i64` comme le reste du schéma — écart vs
  l'`uuid` du §5), dédup par hash, 409 optimiste, rattachement device avec
  garde de puce, suppression refusée tant que des devices sont rattachés.
  Catalogue D92 = `pnex_core::firmware::LIB_CATALOG` (source unique front +
  back, versions exactes vérifiées sur le registre PIO) plutôt qu'un
  fichier TOML.
- **L4 partiel** : `pnex_firmware_builder::run_build_with` / `compile_check`
  (injection du `main.cpp` + `lib_deps`, diagnostics gcc D90) ; bouton
  « Vérifier » = compile sans secrets, registre de checks en mémoire du
  nœud API (2 en parallèle, TTL 1 h).
- **L5 — IDE** : page `/firmware` (menu Edges), mode C++ de l'éditeur
  partagé, bibliothèques, référence API, devices (commandes annoncées),
  historique. E2E Playwright réel : création → lib DHT → compile OK → erreur
  projetée ligne 74 → garde « include absolu » → historique.
- Réglages : `settings.firmware.custom.{enabled,sandbox,bwrap_cmd,allow_network}`
  + env `PNEX_FIRMWARE_CUSTOM_ENABLED` / `PNEX_FIRMWARE_SANDBOX`. Désactivé par
  défaut (409 `firmware-custom-disabled`).

**Évolutions du 2026-09-29 (retours utilisateur)**

- **Rattachement retiré** : un device choisit son firmware **au
  provisioning** (étape Modèle du wizard, projets filtrés sur la puce de la
  carte) — `CreateDevice.firmware_project_id`. Un firmware, N devices. Le
  build (auto au provisioning, ou Rebuild) compile la **dernière révision**
  du projet et estampille `build_records.firmware_revision_id`. Suppression
  d'un projet refusée tant que des devices l'utilisent.
- **« Vérifier » passe par la file de jobs** (worker `firmware_check`,
  table `firmware_checks`, migration 000039) : l'image `pnex-server` n'a
  pas PlatformIO, seul `pnex-builder` l'a.
- **Éditeur** : panneaux latéraux remplacés par les menus « + Lib » et
  « + PneX API » de la barre ; libs du projet en pastilles supprimables.
- `compose.app.yaml` : feature active par défaut en stack dev, **sans
  sandbox** (pas de bubblewrap dans l'image, userns bloqués par le seccomp
  Docker) — usage mono-utilisateur uniquement.

**Reste à faire**

1. Sandbox D91 réelle : isolation hors conteneur partagé (worker dédié
   sans secrets DB dans l'environnement, ou bwrap validé sur hôte).
2. ~~**Offline**~~ : livré 2026-10-05 (semis de libs + cores séparés, §8).
3. ~~Nœud flow pour les commandes custom~~ : Device (write), D146 (2026-10-06).
4. Sur la page device : afficher le firmware custom / la révision
   déployée et « mise à jour disponible » (D94).
5. E2E matériel : BME280 I2C → O2 → flow → notification.
6. **Commande perdue sans trace (constaté 2026-10-06, tuto LED RGB)** : un
   `power = 0` écrit sur le contrôle (valeur bien stockée) n'a pas éteint
   la LED de `climate-1` ; la métrique `light` est restée à 1, le renvoi
   du même ordre une minute plus tard est passé. Cause non identifiée
   (contrôle → Source contrôle → Device (write) → route interne →
   `ServerMsg::Command` → handler) : en mode run, ni le debug du flow ni
   les logs du runtime ne montrent où le message s'est perdu. À faire :
   tracer la chaîne (résultat de la route interne et `Ack` du device
   journalisés côté serveur), et ne plus considérer une commande comme
   « envoyée » tant que l'`Ack` n'est pas revenu (le nœud n'attend
   aujourd'hui que le push WS).
   **Enquête du 2026-10-06 au soir** (cache Valkey `pnex:last:v1:{org}:{device}:light`
   comme vérité terrain, ~180 ms médiane commande → état publié) :
   2 pertes constatées (21:15:14, 21:56:06), puis 0 sur ~230 commandes
   (route interne directe 0/40, contrôle → flow 0/40, couleur 2 s avant
   0/40, bascules à la seconde autour de la resynchro cluster 0/18).
   Écarté : file du moteur (mpsc borné, contre-pression), coupure de
   l'abonnement Valkey du nœud Source contrôle (abonné continu), reload du
   runtime et resynchro complète toutes les 300 s (artefact identique, flow
   non touché). Seul tronçon sans observabilité : push WS serveur →
   handler du device (les `Ack ok:true` ne sont ni attendus ni journalisés).
   Correctif proposé : suivi des `cmd_id` de commande dans la session WS,
   warn « commande non acquittée » après quelques secondes, puis renvoi
   pour les commandes idempotentes (power/level/color) — jamais pour une
   commande momentanée (bouton), sauf dédup `cmd_id` côté lib.
7. **État réel après une commande (idée utilisateur, 2026-10-06)** : le
   firmware générique PneX remonte déjà l'état réel des pins de sortie
   (digital 0/1, duty PWM). Pour un firmware custom, rien d'équivalent :
   chaque sketch doit publier lui-même une métrique d'état (`light` dans
   le tuto). Piste : la lib renvoie automatiquement, après chaque handler
   réussi, la valeur appliquée de la commande comme état (`state_report`
   par `cap_id` de la commande), pour que la carte du dashboard affiche
   l'état réel sans code en plus, et que la perte d'une commande se voie
   (état ≠ contrôle).

## Journal

- **2026-09-29** — Création : faisabilité validée sur l'existant (build par
  device, lib embarquée, variantes figées, Tier 2) ; identification du
  manque API métriques/commandes et du risque d'exécution de code au build.
- **2026-09-29 (implémentation)** — L1, L3, L5 livrés ; L4 partiel (check
  compile, pas encore le build device) ; E2E navigateur réel. Deux bugs
  préexistants corrigés en chemin : overlay de l'éditeur de code rogné au
  défilement, toasts jamais fermés quand l'appelant se démonte.
- **2026-10-06** — D146 : les commandes custom passent par Device (write)
  (inspecteur, route interne, « Créer le flow » filtré par type de
  contrôle) ; cas d'usage = LED RGB WS2812 du C6-Zero pilotée depuis un
  dashboard (tutoriel).
