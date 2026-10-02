# Modèle edge — taxonomie IoT, firmware en couches et collecteurs (D44–D48)

> **Statut : DOCTRINE (réflexion tranchée le 2026-09-13, implémentation par
> phases — §11).** Référence pour tout nouveau développement edge : taxonomie
> (D44), contrat des firmwares autonomes (D45–D46), manifeste de capacités
> (D47), pattern « collecteur » (D48) qui unifie extension Chrome, agent
> local et app mobile. Sources amont : `docs/architecture/control-cards.md`
> (D20), `docs/architecture/firmware-build.md`, `docs/architecture/brick0.md`
> (D17), `docs/inventory.md` (registre), contrat fil
> `crates/pnex-core/src/proto.rs`.

## 1. Le problème : la taxonomie actuelle mélange trois axes

« sensor / actuator / mixte / autonome » confond trois dimensions
indépendantes :

| Axe | Question à laquelle il répond | Exemples |
|---|---|---|
| **Plateforme** | sur quel hardware, comment lire GPIO/ADC/I2C | esp8266, esp32, esp32-s3 |
| **Capacité sémantique** | ce que le device *offre* | `temperature`, `relay`, `counter` |
| **Profil** | **qui porte la boucle de décision** | esclave (serveur pilote) vs autonome (device régule) |

Preuves dans le code actuel :

- `firmware/generic_esp8266` (459 l.) et `firmware/soil_sensor` (472 l.)
  dupliquent ~70 % (WiFi, WS, ChaCha, PING/PONG, backoff, announce/ack,
  framing JSON) — leur différence réelle est le **profil** et la
  **capacité**, pas du code ;
- `4_chan_relay` parlait un **troisième dialecte** (nanopb vs ArduinoJson) —
  protocole mort avec lui (déprécié D20, **supprimé le 2026-09-13** : le
  générique couvre l'usage, cf. §12) ;
- le contrat fil n'a pas de vocabulaire au-delà des modes de pin
  (`digital_in/out`, `adc_in`) ; `ControlConfig` (D20) est le premier
  message *sémantique* — l'ontologie qui manque est justement le chantier.

## 2. La taxonomie (D44)

- **Capacité** = contrat sémantique versionné
  `{id, famille, unité, schéma de config optionnel}`. Trois familles :
  - **measurement** — valeur continue horodatée : `temperature`,
    `humidity`, `soil_moisture`, `adc_raw` ;
  - **state** — discret : `digital_state`, `relay_state`, `counter` ;
  - **actuator** — configurable : `relay_cmd`, `regulation` (le schéma de
    config D20 devient celui de cette capacité).
- **Driver** = implémente 1..n capacités (SHT31 → temperature + humidity ;
  relais → relay_state + relay_cmd ; GPIO nu → digital_state). La
  plateforme n'existe que dans le driver.
- **Profil** = position sur les axes « boucle » × « buffer » + obligations
  de cycle de vie (§4).

Les ids de capacités respectent le canon D16 (`[a-z0-9_:]`).

**Payoffs** (ancrés dans ce qui existe) :

- **Séries O2 stables sémantiquement** : la série suit
  `{org}/{device}/{capacité}` — le gpio devient un détail d'implémentation,
  re-câbler un pin ne casse ni les courbes ni les dashboards ;
- **Palette du flow editor par capacité** : les cartes référencent
  (device, capacité) au lieu de (device, label de pin) — les formulaires
  D20 font déjà la résolution via `device_capability_instances`
  (control-cards.md §4), ils passent du label de pin à la capacité ;
- wizard et dashboards templates par capacité ; versioning **additif
  only** (école CONTRACT) : `capabilities.v1`, jamais de renommage.

## 2 bis. Familles produit de devices (2026-10-01)

La taxonomie D44 (capacité / driver / profil) décrit le **contenu** d'un
firmware. Côté produit, l'utilisateur choisit d'abord une **famille** :

| Famille | Ce que l'utilisateur fait | Code | Modèles du catalogue |
|---|---|---|---|
| **Générique PneX** (entrées/sorties) | Configure ses pins depuis l'UI (modes, write, subscribe, PWM) | Aucun — firmware générique compilé par device | `generic_esp8266`, `generic_esp32c3`, `generic_esp32`, `generic_esp32s3` |
| **IDE custom** | Écrit son `main.cpp` (métriques, commandes) dans l'IDE | Projet firmware versionné, compilé par le serveur (D87–D94) | Un projet choisi à l'étape Modèle, sur la carte d'un générique |
| **Prédéfinie** (prête à l'emploi) | Branche une carte typée et fonctionnelle dont PneX garantit la compatibilité ; les fonctions sont prédéfinies | Firmware dédié maintenu par PneX, **imposé par le modèle** : aucun choix firmware ↔ carte | `soil_sensor`, `generic_esp32cam` (vidéo) ; à venir : cartes de régulation (D20, F3 `regulator`) |
| *Hors cartes* — **agents** | Installe un binaire sur un PC / Raspberry Pi | Agent edge (D95–D99) | `edge_agent` |

Règle de la famille prédéfinie (décision utilisateur 2026-10-01) : le
firmware d'une carte prédéfinie **n'est pas géré par l'utilisateur**. Il
figure dans la liste des firmwares disponibles, rattaché à sa carte
compatible, en lecture seule ; l'assistant n'offre **aucune sélection** de
firmware pour ce modèle (le couple firmware ↔ carte est figé par le
catalogue). Les mises à jour viennent des versions PneX (rebuild / OTA).

Points ouverts (constatés au 2026-10-01, à trancher) :

1. **`custom_device` (Tier 2 local)** : retiré de l'assistant mais encore au
   catalogue et accepté par l'API ; un build serveur est accepté puis échoue
   dans le worker (pas de projet `custom_device` dans `firmware/`), et sa
   page détail rend une carte vide. Aucune famille ne l'accueille.
   **Fait (2026-10-01)** : retiré de la fixture du catalogue (plus seedé,
   donc plus créable). **Fait (2026-10-02)** : code Tier 2 purgé — plus
   de repli « pas d'overlay » à l'admission (4007 au connect sans overlay
   ni firmware custom), mesures dynamiques réservées aux agents, exemple
   `CustomDevice` supprimé, tests migrés. Corrigé au passage : un device
   à firmware custom est admis par les pins de son sketch (zéro pin =
   pin map vide), jamais par l'overlay du modèle générique, qui remettait
   tous les pins en `digital_in` à chaque announce (I2C compris).
2. **Carte `generic`** : absente de `mcu.yaml`, créée à la volée par le seed
   (`soc = generic`) pour `custom_device` et `edge_agent`. Un agent n'a pas
   de carte : `board_id` nullable pour la famille agents plutôt qu'une
   carte fantôme.
3. **Nommage** : `generic_esp32cam` porte « Generic » mais relève des
   cartes prêtes à l'emploi (fonction vidéo prédéfinie).
4. **IDE custom ↔ famille** : la garde actuelle ne vérifie que la puce ; un
   projet peut donc être attaché à `soil_sensor`, et l'assistant affiche le
   choix de firmware pour tout modèle dont la carte a un profil. Contraire
   à la règle ci-dessus : sélecteur et garde serveur à restreindre à la
   famille générique. **Fait (2026-10-01)** : `pnex_core::DeviceFamily`
   (source unique front/back), garde serveur 400
   `firmware-family-locked`, sélecteur affiché pour la famille générique
   seulement.
6. **Liste des firmwares** : la page `/firmware` ne liste que les projets
   de l'IDE ; les firmwares prédéfinis (et le générique) y figureront en
   lecture seule avec leur carte compatible.
5. **Assistant** : une seule section « Traditionnel (strict) » regroupe
   tout le catalogue. **Fait (2026-10-01)** : une section par famille
   (Générique PneX, Cartes prédéfinies, Agents). La famille d'un modèle
   est une liste en dur (`GENERIC_IO_PREDEFS`) ; une colonne `family` au
   catalogue la rendrait pilotée par les données.

## 3. Firmware en couches (toujours PIO)

```
firmware/
  common_libs/
    pnex-transport/   ← WiFi+WS+ChaCha+backoff+announce/ack (dupliqué aujourd'hui)
    pnex-core-cpp/    ← miroir C++ de pnex-core (proto + control, golden vectors)
    pnex-caps/        ← drivers sémantiques : SHT31, DS18B20, relais, compteur…
    pnex-hal/         ← divergences ONLY : ADC res, WDT, deep sleep, stockage, millis
  soil_sensor/ generic/ regulator/ …   ← un projet = plateforme × profil × config device
```

Règles :

1. **Arduino = HAL de facto** — GPIO/I2C/ADC identiques partout ; la
   couche HAL ne contient que les points de divergence (résolution ADC,
   WDT, deep sleep, stockage flash, wrap de `millis()`).
2. **pnex-core-cpp = source de vérité miroir** — le pattern golden vectors
   (tests Rust = tests C++) prévu par D20 §6 s'étend aux descripteurs de
   capacités.
3. **Composition à la compilation** (décision du 2026-09-02 conservée) :
   une env PIO = plateforme × profil × config device. Pas de plugins
   runtime ; le manifeste est compilé dans le .bin.

## 4. Les profils — le contrat firmware

| Profil | Boucle | Buffer | Config persistée | Obligations | État |
|---|---|---|---|---|---|
| `pin_slave` | serveur | non | non (re-cast) | safe-states à chaque perte de lien, backoff | ✅ `generic_esp8266` |
| `regulator` | device | oui | **oui (D45)** | safe capteur muet > timeout, golden vectors `pnex_core::control`, buffer+flush | D20 phase suivante |
| `logger` | aucune | oui | n/a | buffer+flush, cadence locale | point d'extension documenté |

Le `logger` est la mesure pure autonome (rien à réguler, rien à piloter) —
il ne demande rien de plus que `regulator` sans la boucle. **Décision
2026-09-13 : point d'extension documenté, aucune phase ne le porte** ;
premier besoin réel = implémentation.

## 5. L'autonomie — persist ou safe ? (D45)

La doctrine actuelle « re-cast à l'announce, sans EEPROM » reste vraie
**pour le pin_slave**. Pour le `regulator` (et tout profil autonome), elle
ne suffit pas : **reboot pendant une coupure internet = boot sans config →
safe → chauffage coupé en plein hiver**. Décision :

- **Le `ControlConfig` est persisté en flash** (NVS esp32 / LITTLEFS+struct
  esp8266, via pnex-hal). Le boot autonome restaure la dernière config
  connue et **reprend la régulation immédiatement**, avant toute
  reconnexion ; le re-cast à l'announce devient une **conciliation**
  (version de config dans la persistance ; le serveur fait autorité au
  retour du lien).
- **Store-and-forward** : ring buffer en flash borné (24 h @ 1 min ≈ 1 440
  samples × ~30 o ≈ 43 Ko — trop pour la RAM esp8266), flush au reconnect
  **idempotent** (clé `(boot_id, seq)`, D46), throttlé quand le lien est
  down.
- **Horloge** : `millis()` monotonic (wrap unsigned déjà correct dans le
  code actuel). **Pas de RTC ni de SNTP obligatoires** — la datation est
  reconstruite côté serveur (D46). SNTP reste un bonus (D12.d) jamais une
  dépendance.
- **Safe-states inchangés** (control-cards.md §6) : sortie safe seulement
  sur capteur illisible/hors plage > `data_timeout_secs` ou boot **sans**
  config persistée (premier boot). « WS down ≠ arrêt de régulation ».

## 6. Le temps des échantillons bufferisés (D46)

Un régulateur subit une coupure de 6 h en bufferisant à 1 min → ~360
samples au flush. À quelle heure les tracer ?

- **Dédup idempotente** : clé `(boot_id, seq)` — `boot_id` tiré au boot,
  `seq` monotone par boot. Un flush ré-essayé ne crée pas de doublon.
- **Reconstruction** (décision 2026-09-13) :
  `capture ≈ arrival − (arrival_uptime − capture_uptime)` — l'uptime
  `millis()` voyage dans chaque sample, la cohérence est vérifiée contre la
  cadence souscrite ; au-delà d'une fenêtre de confiance, **trou marqué**
  (jamais de courbe inventée).
- **Alternative rejetée** : heure d'arrivée seule → plat mensonger pendant
  la coupure puis saut au flush.
- **S'appuie sur D12** : le timestamp device optionnel + `ts_source`
  (provenance) existent déjà — le buffering ajoute la reconstruction par
  dérive d'uptime et la clé de dédup. `ts_source` gagne une valeur
  (`device` avec reconstruction), le backend reste le point de datation.

## 7. Le manifeste de capacités (D47)

**Qui fait autorité ?** Conséquence directe du compile-per-device : quand
le serveur compile le firmware, il connaît déjà le manifeste (il est dans
la config de build).

| Firmware | Rôle du manifeste dans l'announce |
|---|---|
| Compilé par le serveur | **Attestation** — check de cohérence (fw version, hash de config). Le serveur est autoritatif. |
| Custom (user) | **Découverte** — le manifeste EST la source, validé contre les déclarations de board/org. |

**Écran de debug local (2026-09-20)** : l'écran n'est **pas** une famille de
capacité (measurement/state/actuator) — il reste un **périphérique device**
(`device_registries.peripherals`) + **périphérique profil board v2**
(`ScreenPeripheral`, `builtin` = soudé forcé) + **define de compilation**
(`PNEX_SCREEN_*`, contract firmware-build.md §2.1.1). L'announce peut
l'attester : cap `{"id": "screen", "family": "display"}` émise quand un
driver est compilé (`pnex_screen` de la lib PneX).

**Trajectoire (bascule douce)** : le board overlay reste l'autorité du
pin-slave existant — pas de migration forcée ; le manifeste devient
l'autorité pour le sémantique et le custom.

**Contrat fil — additif** (CONTRACT intact) :

- `Announce` gagne `caps: [...]` (manifeste versionné) ;
- `StateReport` étendu : `{cap_id, metric, value, uptime_ms, boot_id, seq}`
  en complément du couple `{gpio, value}` ;
- `ControlConfig` devient un cas particulier d'un Config par capacité ;
  `RegState` routé vers O2 (lève la limite « journalisé, pas encore routé »
  du §8 de control-cards.md).

## 8. Les collecteurs (D48)

> **AMENDEMENT 2026-09-15 (décision user, direction corrigée)** : pas de
> domaine dédié source/binding — la tentative C1 « web_source + bindings +
> tokens + UI » a été **revertée** (jamais poussée). Deux types à la place :
> **(a)** le cas courant = un simple **nœud de flow** HTTP configurable
> (méthode GET/POST, headers, auth basic/bearer/clé, **proxy configurable**
> — direct ou via un provider de scraping type ScraperAPI/ZenRows/Zyte),
> la donnée traversant le moteur ETL standard vers les séries O2 ;
> **(b)** les pages JS lourdes = une **extension navigateur (Chrome/Firefox)
> « type device »** : petite page de login, sync des règles écrites dans
> PNeX (desired-state, école D20), exécution in-browser, retour par le
> chemin d'ingestion devices existant. L'extension réutilise l'identité et
> le pattern devices au lieu du modèle source ≠ binding décrit ci-dessous
> (§8.1–8.3 : le vocabulaire source/binding reste valide comme lecture de
> la doctrine initiale, l'implémentation passe par le modèle device).

Aucun de ces clients n'est un device (pas de pins, pas de build firmware,
pas de safe-states). Pattern commun, même forme que les devices :
identité, config poussée (desired-state), ingestion → **séries O2**,
diagnostics.

| | Firmware | App mobile | Extension | Agent |
|---|---|---|---|---|
| Contexte | MCU non supervisé | poche de l'user | navigateur de l'user | machine de l'user |
| Confiance | totale au serveur | user via OIDC | **user-délégué** | user via token |
| Présence | connexion WS permanente | à l'usage | éphémère (navigateur ouvert) | daemon + file disque |
| Survit coupure réseau | profils autonomes : oui | n/a | non (léger) | oui |

### 8.1 Source ≠ binding

- **Source** (`web_source`, `agent_source`) = resource **org**, comme un
  device : ses séries O2, ses tâches (config), ses dashboards.
  Org-scoped, partageable.
- **Binding** = (instance de collecteur, user, token, tâches approuvées
  *avec version*). C'est le « hardware » du collecteur — le navigateur ou
  la machine de l'utilisateur. **Le consentement vit dans le binding,
  jamais dans la source.**

Le parallèle est exact : un device est une resource org (D42), la
*connexion physique* porte le token ; ici la connexion physique = l'instance
navigateur/agent de l'user. Conséquences :

- liveness à deux niveaux (extension `device_liveness.rs`) : **binding en
  ligne** (check-in récent) vs **source saine** (≥ 1 binding actif qui
  pousse) ;
- tokens par binding, révocables un à un ; key id dans le format de token
  (rotation de clés différée mais prévue) ;
- tâche versionnée : `{desired: v3, approved: v1}` par binding → l'UI
  montre « cette extension tourne sur une version périmée de la tâche ».

### 8.2 Extension Chrome — user-déléguée, jamais serveur-déléguée

L'extension ne rapporte que ce que le navigateur **de l'utilisateur**
rend, quand son navigateur est ouvert, à cadence modeste. Le jour où le
serveur pourrait orchestrer du fetch massif, on change de catégorie
(scraper distribué — ToS des sites). Décisions :

1. **Un sélecteur CSS = un pin.** Chaque champ extrait devient une série
   O2 comme la valeur d'un pin — même API series, même page dashboard,
   même entrée de flow. Le payoff produit : un prix de marché devient une
   donnée télémétrique comme une sonde d'humidité.
2. **Tâche** = `{url/pattern, cadence ≥ 30 s + jitter, sélecteurs → champs
   nommés, hash → détection de changement, mode payload}` — authoring UI
   PNEX, cast vers l'extension (doctrine desired-state D20).
3. **Tab-open only en v1** — le content script ne vit que quand l'onglet
   est ouvert. Le fetch service worker (cookie jar, sans onglet) est une
   v2 à activer explicitement.
4. **Cast ≠ apply** — l'asymétrie de principe : device fait confiance au
   cast (sécurité physique) ; extension exige le **consentement user par
   tâche** et par site (`optional_host_permissions`, geste utilisateur).
   Règle cohérente : sécurité physique → confiance serveur ; données perso
   → souveraineté user.
5. **MV3 structure la cadence** : service worker non persistant → boucle =
   `chrome.alarms` (min 30 s, + jitter) — le navigateur impose
   structurellement le « léger ». **Config-as-data, jamais config-as-code**
   : les tâches/sélecteurs poussés par PNEX sont des données interprétées
   par le code bundled (règle MV3 « no remote code ») — pas d'eval, pas
   d'injection de script distant.
6. **Minimisation** : mode « fields » par défaut (sélecteurs → champs) ;
   HTML de page complète = opt-in rare, plafonné, traité comme sensible
   (contient du contenu authentifié de l'user).
7. **Auth : token par binding** — jamais le JWT Rauthy user
   (sur-privilégié + refresh dance douloureuse en service worker).
8. **Cycle d'échec** : sélecteur cassé (redesign) → sample à champs
   absents → séries trouées marquées, tâche **degraded** ; N échecs
   consécutifs → auto-pause côté binding + alerte UI ; **jamais
   d'auto-réactivation silencieuse** ; rate-limit détecté (429/captcha) →
   backoff obligatoire — le « léger » est aussi une défense du site cible.
   Install CWS plus tard ; sideload dev-mode pour la communauté
   self-hosted d'abord.

### 8.3 Agent local — le jumeau réseau du device autonome

> **Livré** (2026-09-30) : `edge-agent.md`, D95–D99. L'implémentation passe
> par le modèle device (comme l'extension) : un agent = un device
> `edge_agent`, ingestion libre, Valkey toujours, O2 en opt-in par clé.

- Binaire **Rust** (réutilise `pnex-core`), serveur HTTP **127.0.0.1** pour
  les producteurs locaux (scripts, LAN) ; bind LAN = opt-in avec token
  (TLS/mTLS différé).
- **File disque persistante + retry/backoff** — même doctrine
  store-and-forward que le firmware (D46 s'applique tel quel : `(boot_id,
  seq)` + reconstruction).
- Token par binding, config poussée depuis PNEX (desired-state).
- Futur : pont WS pour les devices LAN (symétrie avec
  `firmware/ws-server/cast_server.py`).

### 8.4 App mobile — thin client sur l'API existante

- OIDC Rauthy + API existante (aucun nouveau contrat d'ingestion).
- **QR = `{host, device_id, pair_code à usage unique}`** échangé par une
  app authentifiée — **jamais le token device en clair dans un QR**
  (imprimable, copiable).
- GPS → `device_placements` (D43) et `device_positions` (D38, source
  `manual`).

## 9. OTA — implémenté (2026-09-22)

Aucun régulateur déployé au champ ne se reflashera par Web Serial
(firmware-build.md §4). **Réservations du 2026-09-13 consommées** :
pull HTTP(S) du .bin versionné + sha256 (serveur), attesté par la cap
`ota` du manifeste F2, inventaire « le serveur sait quelle version tourne
où » (`device_registries.fw_version`). Détail complet (machine à états,
modèle eboot 8266, stratégie TLS/CA, checklists matérielles) :
**`docs/architecture/ota.md`**.

## 10. Sécurité transverse

- **MCU** : ChaCha20-over-WS (D8) inchangé ; TLS `wss` en industriel.
- **Collecteurs** : TLS + token par binding ; scope = « write samples sur
  SA source » ; key id dans le token (rotation différée, prévue).
- **Jamais de JWT user dans les collecteurs** (sur-privilégié, refresh
  dance) — sauf l'app mobile qui est un client API *au nom de l'user*
  (OIDC) et non un collecteur push.

## 10 bis. Ce que ça touche côté code

- `crates/pnex-core/src/proto.rs` : `Announce.caps`, StateReport étendu,
  Config généralisé (voir §7) — additif, CONTRACT intact ;
- `services/telemetry.rs` : routage par capacité en plus du gpio ;
- `device_liveness.rs` : étendu aux bindings collecteurs ;
- nouveau service/controller `collectors` + migrations (`web_sources`,
  `collector_bindings`, `collector_tokens`) ;
- `firmware/common_libs/` : les couches du §3.

## 11. Roadmap proposé

1. **F1** — facto transport commune (`pnex-transport`) : refactor pur,
   CI « une version pnex = un firmware qui compile » verte, zéro changement
   de comportement. **✅ Fait (2026-09-13)** : `common_libs/pnex-transport/`
   (config b64, WiFi, WS, framing ChaCha, bookkeeping PONG + timeout) ;
   `generic_esp8266` et `soil_sensor` réécrits dessus — policy (backoff,
   safe-states, retries, announce, affichage) restée dans chaque main ;
   `tft_dev` hors F1 (`4_chan_relay` supprimé le 2026-09-13). Règle de
   linkage : `config.h` n'est plus inclus que par `pnex_transport.cpp`
   (globals non const = dupliqués au link sinon).
2. **F2** — manifeste de capacités + StateReport généralisé (additif
   contrat). **✅ Fait (2026-09-13)** : `Announce.caps`
   (`CapDesc{id, family, unit?, version}`, attestation — l'overlay board
   reste l'autorité du pin_slave, serveur journalise qui tourne avec quoi,
   réservation OTA §9) ; `StateReport` + `cap_id`/`uptime_ms`/`boot_id`/
   `seq` (routage par capacité **livré et testé** côté serveur — série
   `{org}/{device}/{capacité}`, source_type `generic_cap` — activation au
   premier firmware par capacité F3 ; le pin_slave n'émet pas `cap_id` :
   deux pins digital_in partagent `digital_state`) ; `RegState` routé vers
   O2 (`<node>_sensor/_out/_cycles`, lève la limite §8 de control-cards.md ;
   NaN = pas de point) ; fil fixé : « null » (ArduinoJson) désérialise en
   NaN pour `RegDiag.sensor_value` (avant : RegState capteur illisible
   droppé au parse). Différé : checks d'attestation durs (inventaire
   versions) et `ControlConfig` cas particulier d'un Config généralisé
   (F3).
3. **F3** — firmware `regulator` (D20 phase suivante) : prouve l'autonomie
   (persistence D45, buffering D46, golden vectors control).
   **Fondations posées (2026-09-13)** : `pnex-core-cpp` (miroir C++ de
   `pnex_core::control`) + golden vectors générés par
   `crates/pnex-core/tests/golden_vectors.rs` (JSON + `goldens.h`,
   regen `PNEX_REGEN_GOLDENS=1 cargo test -p pnex-core
   --test golden_vectors`) et rejoués sur l'hôte par
   `firmware/core-cpp-tests` (`pio test -e native`, étape CI firmware) —
   le miroir ne peut pas dévier de la référence sans casser la CI.
   Reste : le firmware `regulator` lui-même (persist D45, buffer D46,
   boucle) — hardware requis.
4. **C1** — collection web, **deux types** (amendement 2026-09-15,
   remplace la version « domaine dédié » revertée) :
   - **C1a — nœud flow `http_fetch`** ✅ (2026-09-15) : bloc ETL
     configurable (méthode GET/POST, headers, auth basic/bearer/clé API,
     proxy — direct ou provider de scraping ScraperAPI/ZenRows/Zyte/…),
     sortie = message standard du moteur → séries O2 ; zéro table, zéro UI
     dédiée (crate `pnex-node-http-fetch` + kind `http_fetch`).
   - **C1b — extension navigateur « type device »** : **spécification
     écrite** ✅ (2026-09-15) — `docs/architecture/extension-collector.md`
     (10 décisions tranchées : WS device unique, crypto WASM pnex-core,
     entité `extension_rules`, variant `ServerMsg::RulesConfig`, OIDC pour
     le plan gestion / device token pour le plan données, connect-on-wake,
     cast ≠ apply). Implémentation = jalon C3, après validation de la
     spec.
5. **C2** — agent local (premier client Rust) — **livré** 2026-09-30, cf. `edge-agent.md` (D95–D99).
6. **C3** — extension Chrome/Firefox : implémentation de la spec C1b
   (`extension-collector.md`), MV3 + consentement par tâche.
7. **A1** — app mobile (thin, API existante).
8. **M1–M3 — mode M2M résilient distribué** (nouvelle piste, PRD :
   `docs/architecture/m2m-resilient.md`, statut proposition) — boucle
   sonde → décision → actionneur **en pair-à-pair** (pub/sub zenoh-pico)
   qui survit à la perte du serveur. **École volontairement distincte de
   la doctrine serveur-centrique de ce document** (autorité locale, pas de
   lien serveur dans la boucle, contrat de compatibilité M2M distinct du
   CONTRACT — contraste doctrine par doctrine dans le PRD §2 bis) ;
   devices spécialisés à contrat strict (sonde, actionneur, passerelle),
   hub = **config globale** (cast au changement + sync périodique,
   fréquence à définir) + collecte uniquement — la logique métier vit
   dans le nœud, fail-safe local.
   **M1** POC WiFi zenoh-pico sans broker (parc ESP existant, zéro achat —
   coupure hub → la boucle continue) ; **M2** mesh OpenThread/802.15.4
   auto-cicatrisant sous zenoh-pico (ESP32-C6/H2, Thread Border Router en
   hub — risque n°1 : zenoh-pico sur 6LoWPAN, MTU ~1280 o, à prototyper
   avant engagement) ; **M3** industrialisation (provisioning zéro-touch,
   clés, OTA §9, versionnage du contrat ; UI de gestion à prévoir à cette
   étape — doctrine « UI = seule interface user »).

## 12. Ouvertures (hors périmètre immédiat)

- **Palette flow par capacité** — l'ontologie D44 rend les cartes
  référençables par (device, capacité) ; à faire au moment où le flow
  editor touche aux formulaires D20.
- **Protobuf/flatbuffers pour le fil MCU** — ArduinoJson reste le dialecte
  de référence (école generic), nanopb est mort avec `4_chan_relay`
  (D20, supprimé 2026-09-13).
  Une compression du fil est une ouverture, jamais un prérequis.
