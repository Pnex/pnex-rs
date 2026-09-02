# Brick 0 — Firmware générique ESP8266 + socle capabilities

> **Statut : PRD ancré au repo (2026-09-02) — prêt pour implémentation.**
> Le PRD d'origine (revue conversationnelle) est résolu par les décisions
> ci-dessous ; ce document fait foi. Docs liés :
> `docs/contracts/ws-sensor-ingest.md` (patterns WS réutilisés tels quels),
> `docs/architecture/firmware-build.md` (pipeline de build),
> `docs/inventory.md` D13/D15/D16/D17.

## 0. Décisions actées (2026-09-02)

| # | Décision | Motif |
|---|---|---|
| B0.1 | **Artefact générique = un `.bin` + config injectée au flash.** Zéro compilation par provisioning : le wizard collecte WiFi/host/token/device_id comme aujourd'hui, mais l'artefact = firmware générique (compilé une fois par version de source) + un secteur de config flashé à offset dédié | Le workflow custom actuel (recompiler à chaque provisioning, secrets au build) est trop compliqué — c'est le problème que Brick 0 supprime |
| B0.2 | **Workflow inchangé** : enregistrement via le wizard existant (device pré-créé dans `device_registry`, token + clé ChaCha20 générés comme aujourd'hui) → flash → le device s'annonce sur `/ws/device` → validé contre la ligne existante. **Pas d'auto-création à l'annonce** | Même workflow qu'avant, sinon incompréhensible. L'auto-création casserait le scoping org (à qui rattacher le device ?) |
| B0.3 | **`device_profiles` différé à P5** (avec les policies `Profiled`/`Locked`). P0 = policy `Validated` uniquement : admission des caps validée contre chip-caps + overlay board | La table ne sert qu'à `Profiled` ; l'ajout ultérieur est trivial (table + check d'allowlist) |
| B0.4 | **ChaCha20 nu dès P0** — framing `base64(nonce(12)‖ct)` identique à `/ws/sensor/ingest`, clé = `device_tokens.encryption_key`, code `common_libs/crypto` réutilisé tel quel | Déjà en place des deux côtés ; le stage « plaintext d'abord » du PRD n'économisait rien |
| B0.5 | **Le `Write` reste en P0, borné au manuel** : action utilisateur depuis l'UI (toggle) + provisioning/config push. **Jamais de boucle de régulation serveur** → consigné D17 (frontière avec le chantier M2M, D13) | Must du prototypage rapide sur cartes de dev ; la régulation reste à l'edge |
| B0.6 | **Extension du modèle existant, pas de registre parallèle** : `device_registry` / `device_tokens` / `device_states` réutilisés tels quels ; capabilities *instances* = nouvelle table ; overlay board = data (`mcu_boards.details`) ; chip-caps = code (`pnex-core`) | Modèle « sans copies » (directive user) ; les tables `devices`/`capabilities` proposées par le PRD doublaient l'existant |

## 1. Modèle de données (extension de l'existant)

Trois niveaux — **channel → capability instance → (profile différé)** :

| Niveau | Porte quoi | Où ça vit |
|---|---|---|
| **Chip-caps ESP8266** (silicium) | GPIO6–11 interdits (flash SPI), strapping GPIO0/2 = HIGH au boot, GPIO15 = LOW au boot, GPIO16 (pas d'interrupt/pwm, pulldown only), A0 canal unique 10-bit | **Code** — `pnex-core` (~20 lignes de table de contraintes) |
| **Overlay board** (câblage NodeMCU/D1 mini) | labels D0…D8/A0 → GPIO, LED onboard (GPIO2, active-LOW), pull par défaut, safe-states | **Data** — `mcu_boards.details` (JSONB déjà en schéma, jamais seedé) + fixture YAML seedée |
| **Capability instance** (état live d'un pin d'un device) | mode courant, config, snapshot des contraintes validées | **PG** — nouvelle table `device_capability_instances` |

Migration 000008 :

```
device_capability_instances
  id · device_registry_id (FK cascade) · gpio (u16, adressing fil) ·
  label ("D1"/"A0", dénormalisé pour l'affichage) ·
  mode (enum digital_in | digital_out | analog_in) ·
  config (jsonb : pullup, safe_state, interval_ms…) ·
  constraints_snapshot (jsonb : ce qui a été validé à l'admission) ·
  enabled · created_at
  unique (device_registry_id, gpio)
```

- **Pas de colonne `role`** : sensor/actuator se dérive du mode en code
  (`digital_in`/`analog_in` → sensor, `digital_out` → actuator) — modèle
  sans copies. Le changement de mode à la volée bascule le rôle.
- Le device **pré-enregistré** (`device_registry`) pointe vers un nouveau
  predefined device **`generic_esp8266`** (board `esp8266` déjà seedé,
  device_type `mixed`) — c'est le modèle à choisir dans le wizard.
- L'admission (policy `Validated`) crée les instances à l'`Announce` :
  le serveur dérive la carte de pins de l'overlay, valide chaque pin
  contre les chip-caps, persiste.

## 2. `pnex-core` — types + validation (source de vérité)

```
pnex-core/src/
  proto.rs        # messages fil (§3), serde tag "t" — natif + wasm32
  caps.rs         # ChipCaps ESP8266 : validate(channel, mode, config) -> Result<_, Violation>
  boards.rs       # types d'overlay board (PinMap, désérialisable depuis mcu_boards.details)
```

- `caps::validate` = le **point unique** de validation, utilisé par le
  backend à l'admission **et** avant chaque push de commande. Règles P0 :
  GPIO6–11 interdits ; `safe_state: high` refusé sur GPIO15 ;
  pull-up refusé sur GPIO16 (pulldown only) ; A0 = `analog_in` only.
- L'ESP8266 ne compile pas de Rust : le firmware **miroire** les types fil
  (ArduinoJson). Le contrat = le schéma fil, pas le partage de code (§2.4
  du PRD, inchangé). Le firmware n'a **pas besoin** de l'overlay : le
  serveur envoie des numéros GPIO + labels dans `ProvisionAck`/`SetMode`
  (RAM 8266 ménagée).

## 3. Protocole fil — WS `/ws/device`

Framing **identique à `/ws/sensor/ingest`** : auth query b64
(`token` + `device_id`), frames texte `base64(nonce‖ChaCha20)`,
`PING`/`PONG` au niveau frame (code `common_libs` partagé). Messages
métier en JSON tagué `t` (serde, puis miroir ArduinoJson) :

```rust
#[serde(tag = "t")]
enum DeviceMsg {                       // device -> serveur
  Announce   { chip: String, board: String, fw: String },   // device_id vient de la query
  StateReport{ gpio: u16, value: Value },                   // ts = ingestion serveur (P0)
  Ack        { cmd_id: Uuid, ok: bool, err: Option<String> },
}
#[serde(tag = "t")]
enum ServerMsg {                       // serveur -> device
  ProvisionAck { caps: Vec<PinSpec> },                      // PinSpec { gpio, label, mode, safe_state }
  SetMode      { cmd_id: Uuid, gpio: u16, mode: Mode, opts: ModeOpts },
  Write        { cmd_id: Uuid, gpio: u16, value: Value },
  Subscribe    { cmd_id: Uuid, gpio: u16, interval_ms: u32 }, // 0 = désabonner
  Reject       { reason: String },
}
```

- `Announce` → service provisioning : policy `Validated` (dérive overlay →
  validate → persiste instances → `ProvisionAck`).
- `StateReport` → `telemetry::sink()` (même sortie que l'ingest : metrics
  OpenObserve, séries `generic_gpio{device_id,…}`, noms normalisés D16) —
  les courbes A0 sont chartables par la page Visualisation existante.
- **Anti-clone / bail** : mêmes mécanismes (`SessionGuard` + fallback
  `device_states` + reaper), mêmes close codes 4001/4002/4003/4005/4006/4008.
  Registre de sessions séparé (`DEVICE_SESSIONS`) avec canal mpsc par
  device = la **downlink** (les commandes REST poussent dedans).
- Branché dans `app.rs` `routes()` à côté de `ws_ingest::routes()`.

## 4. Firmware `firmware/generic_esp8266/`

PlatformIO `espressif8266` / `nodemcuv2` / arduino, `lib_extra_dirs =
../common_libs` (crypto + config réutilisés ; pas d'U8g2/nanopb).
Conventions des firmwares existants reprises : PING 5 s / PONG 15 s,
`ESP.wdtEnable(WDTO_4S)` + `wdtFeed` dans toute attente, `yield()`
régulier, buffers statiques (stack ~4 Ko), backoff reconnect 1 s→60 s.

- **Boot** : lit le secteur de config (§5). Config absente/corrompue →
  LED clignote vite, retry boucle (pas de portail captif — la config
  arrive par le flash, décision B0.1).
- **Boucle** : connecte WS → `Announce` → applique `ProvisionAck`
  (modes initiaux, safe-states) → traite `SetMode`/`Write`/`Subscribe`,
  publie `StateReport`, PING périodique.
- **Perte de lien / PONG timeout / WDT** → toutes les sorties vers leur
  `safe_state` (pattern `forceAllOff()` de `4_chan_relay`).

## 5. Config injectée au flash (B0.1)

- **Offset `0x200000`** (4 Mo), 1 secteur 4 Ko : le `.bin` applicatif
  esp8266 reste < 1 Mo et le SDK ne touche pas cette zone (à re-valider
  au premier flash réel). Pas de filesystem dans le générique.
- Format : magic `PNEXCFG1` + version + CRC32 + JSON compact
  `{wifi_ssid, wifi_password, host, token, device_id, ws_ssl}` —
  **chaînes claires, pas de base64** (la contrainte `-D` du build a disparu).
- **Écriture = 2 entrées dans un seul `writeFlash` esptool-js**
  (firmware@0x0 + config@0x200000) — le glue `flasher.js` accepte un
  tableau ; fallback : image paddée côté worker si esptool-js coinçait.
- `FlashModal` **inchangée** pour l'utilisateur : elle télécharge les
  deux blocs à l'ouverture, un clic = flash complet.

## 6. Backend — routes & service provisioning

| Route | Effet |
|---|---|
| `GET /ws/device` | protocole §3 (`controllers/ws_device.rs`) |
| `GET /api/v1/devices/{id}/pins` | instances + overlay + **last_values** (mémoire de session, `—` si offline) ; viewer inclus |
| `POST /api/v1/devices/{id}/commands` | `{op: "set_mode"\|"write"\|"subscribe", gpio, …}` — `caps::validate` **avant** push (400 + raison si illégal), puis downlink mpsc → `cmd_id` ; écriture owner/admin (`can_write`), lecture viewer |

- `services/provisioning.rs` : admission à l'`Announce` (dérive + valide
  + persiste) — point d'extension unique pour `Profiled`/`Locked` (P5).
- `ensure_token` / `generate_token` / `generate_device_key`
  (`controllers/devices.rs:80-94,247`) passent `pub(crate)` — réutilisés
  tels quels (le wizard crée déjà le device + token).
- Seed : caps `digital_in`/`digital_out`/`analog_in` (si absentes du
  catalogue), predefined `generic_esp8266`, overlay NodeMCU en YAML
  (`fixtures/devices/board_overlay_nodemcu.yaml` → `mcu_boards.details`)
  — overlay **contribuable en data**, jamais en `.h` (§2.3 PRD).

## 7. UI (Dioxus)

Section **« Pins »** dans `DeviceDetail` (pas de nouvelle route — motif
statique-only), visible pour les devices `generic_esp8266` :

- grille de pins depuis `/pins` : label (D1) + GPIO + select de mode
  (`digital_in`/`digital_out`) — apply = `POST commands` si `can_write` ;
- `digital_out` : toggle HIGH/LOW + dernier état ; `A0` : dernière valeur
  + subscribe (rafraîchi par polling 15 s du GET — pattern dashboard) ;
- conventions obligatoires (leçons consignées) : **zéro `set` de signal
  au rendu**, valeurs effectives pour les selects (pattern
  `eff_metric`/`eff_device`), classes Tailwind littérales complètes,
  i18n `brick-*` dans les deux `.ftl` (test de parité).

## 8. Sécurité & safe-states

- Validation serveur **avant** le device (`caps::validate`) — une op
  illégale (GPIO6–11, safe_state qui casserait un boot strapping) est
  rejetée 400 avec raison, jamais poussée.
- Safe-state à la déconnexion (firmware, tous les chemins de perte) +
  boot-safe (interdits strapping à l'admission).
- Auth : device = token par device existant (pas de token partagé —
  mieux que le PRD §8) ; REST = JWT Keycloak + `OrgContext` existants.

## 9. Definition of Done

1. Le wizard enregistre un device `generic_esp8266` ; **aucune
   compilation** : flash depuis l'UI du `.bin` générique + config.
2. Le device se connecte, s'annonce, apparaît **Active** avec la carte
   de pins NodeMCU (labels D0…D8/A0).
3. `D1` (GPIO5) en `digital_out`, toggle HIGH/LOW → LED/réagit.
4. `D2` (GPIO4) en `digital_in` pull-up → lecture reflète un bouton.
5. `A0` lue et rafraîchie dans l'UI (+ visible en série O2).
6. Op illégale (GPIO6–11, safe_state illégal) → rejetée 400 avec raison.
7. Coupure WS → sorties en safe-state.
8. Les devices compilés existants (`soil_sensor`, `4_chan_relay`) ne
   changent pas de workflow.

## 10. Reste ouvert (à trancher à l'implémentation)

- Offset config `0x200000` + sémantique erase : à valider au premier
  flash réel (esptool erase par secteur d'entrée).
- `writeFlash` multi-entrées esptool-js 0.6.1 : vérifier ; sinon image
  paddée worker (le doc §5 part sur multi-entrées).
- Espacement des versions de firmware générique : l'`Announce` porte
  `fw` ; politique de mise à jour (re-flash) à définir quand il y aura
  une v2 du `.bin`.

## 11. Roadmap (PRD §10 re-mappé)

| Phase | Contenu | Statut |
|---|---|---|
| **P0** | ce document | à faire |
| P1 | auth device renforcée / rotation de token ; à reprioriser avec la suite Phase 5 (lecture metrics live) | différé |
| P2 | catalogue board communautaire (l'overlay est déjà en data) | différé |
| P3 | handoff générique → compilé (« ce capteur exige un driver → je te le génère ? ») | différé |
| P4 | générique **ESP32 en Rust** (esp-hal) partageant réellement `pnex-core` ; PWM, I²C/SPI | différé |
| P5 | `device_profiles` + policies `Profiled`/`Locked` ; branchement M2M (D13) | différé |
