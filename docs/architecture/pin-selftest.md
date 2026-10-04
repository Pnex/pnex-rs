# Mode test des pins + banc HIL — exigence obligatoire par carte (D122)

> **Statut : LIVRÉ (2026-10-03)** — T1 à T4 ; T5 (gabarit de bouclage L2)
> reste à faire. S'applique au firmware générique « type Firmata »
> (`firmware/lib/pnex`, Tier 1/2, `generic_esp8266` / `generic_esp32*`).
> Docs liés : `brick0.md` (§8 safe-states, §9 DoD point 9, D121 catalogue
> en code), `firmware-build.md`, `edge-model.md` (F2 StateReport).

## 0. Décision

| # | Décision | Motif |
|---|---|---|
| D122 | **Toute carte du catalogue (`pnex_core::catalog::boards::ALL`) livre un rapport de banc HIL vert** (`firmware/hil/reports/<board>.json`) couvrant chaque GPIO de son profil dans chacun des modes que les chip-caps autorisent. Le rapport contient les **valeurs relues sur la broche** (pas les valeurs commandées) ; le verdict est recalculé en CI à partir de ces valeurs brutes. Garde bloquante : `cargo test -p pnex-core selftest`. | Jusqu'ici une carte entrait au catalogue sur la foi de sa fiche de brochage : les invariants D121 vérifient la *cohérence* du profil, jamais qu'un `write` produit un niveau, qu'une PWM sort, qu'un ADC lit. Les écarts (A0 lu en `digitalRead`, OLED V3 sur D5/D6, TX/RX provisionnés = série muette, I2C matériel inutilisable sur NodeMCU OLED) ont tous été trouvés à la main, après coup. Le premier passage du banc en a trouvé deux de plus (§6). |

## 1. Constat avant D122 (audit du 2026-10-02)

- Journal série des commandes (`[CMD] write GPIOn -> HIGH`) : valeur
  **commandée**, jamais relue.
- `StateReport` : `digitalRead`/`analogRead` réels mais **rien sur la
  série** ; en `pwm_out`, le duty mémorisé.
- `core-cpp-tests` : hôte uniquement (golden vectors), aucun GPIO.
- E2E matériel à la main (pyserial ad hoc), aucun rapport conservé.

## 2. Firmware — valeurs réelles en production (`pnex_io`)

`firmware/lib/pnex/src/pnex_io.{h,cpp}` = **le seul chemin d'E/S des
pins**, partagé par le chemin WS (`pnex.cpp`) et la console de banc : le
banc exerce le code de prod, pas une copie.

- **`[IO] GPIOn mode=… value=…`** à chaque `StateReport` (valeur envoyée
  au serveur = valeur journalisée).
- **Relecture après `write`** : `[CMD] write GPIO4 -> HIGH (readback=HIGH)` ;
  un écart journalise `[IO] MISMATCH GPIOn commanded=… pad=…`. Le
  `StateReport` qui suit chaque `write` porte la valeur relue : le serveur
  (`/pins` `last_value`) voit l'état électrique, sans changement du
  protocole.
  - ESP8266 : `digitalRead` = niveau du pad.
  - ESP32 (core Arduino 2.0.17) : `OUTPUT` = `0x03` (entrée + sortie), la
    relecture est celle du pad ; `pnex_io_level` force l'activation du
    buffer d'entrée (`PIN_INPUT_ENABLE`) sans toucher au routage de sortie.
- **PWM** : la prod rapporte le duty programmé ; la mesure réelle du duty
  (échantillonnage du pad, ~25 ms, bloquant) est réservée au banc.
- **Canaux LEDC gérés par la lib** (ESP32/C3/S3) : allocation/libération
  explicites, depuis le haut (les bas restent au XCLK caméra), 1 kHz /
  8 bits comme `analogWrite`. `set_mode pwm_out` sans canal libre →
  `Ack err "pwm_channels_exhausted"` et le pin repasse en `digital_in`
  (jamais d'ack d'un mode que le matériel n'applique pas). Un re-provision
  libère les canaux de l'ancienne table.

## 3. Firmware — console de banc (`PNEX_TEST_MODE=1`)

Projet dédié `firmware/selftest/` (envs `esp8266`, `esp32`, `esp32c3`,
`esp32s3` ; board via `PNEX_PIO_BOARD`). Le builder serveur ne construit
que les projets `generic_*`/custom : il ne peut pas produire ce binaire ; en
plus, `PNEX_TEST_MODE=1` + `PNEX_OTA_ENABLE=1` = `#error`.

- Hors ligne : ni WiFi, ni serveur, ni config device.
- Console série 115200, une commande par ligne :
  `info`, `mode <gpio> <mode> [safe_high]`, `write <gpio> 0|1`,
  `pwm <gpio> <duty>`, `read <gpio>`, `release <gpio>`,
  `test <gpio> <mode> [safe_high]` (séquence complète d'un pin × mode).
  Modes = modes fil + `digital_in_pullup`.
- Réponses : une ligne `PNEXT {json}` (valeurs brutes uniquement — le
  firmware ne juge jamais). Erreurs : `{"op":…,"err":"<code>"}`.
- Pins protégés (refus `protected_pin`) : ceux qui tueraient le banc
  lui-même — flash/PSRAM et console (UART0 ou USB natif).
- Séquences `test` : entrées = 5 lectures du pad (10 pour l'ADC) ;
  `digital_out` = écrit 0 puis 1, relit le pad (`w0`, `w1`), revient au
  niveau de repos ; `pwm_out` = duty 0/50/100 % et duty **mesuré** sur le
  pad pour chacun ; puis le pin est relâché (entrée flottante, canal PWM
  libéré).

## 4. Plan, verdict, harnais

- **`pnex_core::selftest`** (Rust, source unique des règles) :
  - `plan(board)` : étapes `(gpio, mode)` dérivées du profil + `caps`
    (`available_modes`, pull-up si `validate` l'accepte, niveau de repos
    haut sur strapping boot-HIGH). Exclus : console UART0 (`console`),
    pins d'un écran soudé (`builtin_screen`), aucun mode (`no_mode`).
  - `evaluate` : `digital_in` = niveaux logiques ; `digital_in_pullup` =
    tout à 1 (strapping : niveau libre, résistances de carte) ou **tout à
    0 si le profil déclare `pull_down`** (le banc confirme le câblage) ;
    `adc_in` = dans `0..=adc_max` ; `digital_out` = `w0=0, w1=1` ;
    `pwm_out` = mesuré ≤ 5 / 35–65 / ≥ 95.
  - `check_report` : le rapport couvre **exactement** le plan (pin ajouté
    ou renommé sans re-test = rouge) et chaque étape passe.
  - Exemple CLI : `cargo run -p pnex-core --example selftest -- plan <board>`
    / `-- check <report.json>`.
- **Harnais** `firmware/hil/pnex_hil.py` (`task fw:hil BOARD=… PORT=…`) :
  plan (Rust) → build + flash du banc (**port toujours explicite**) →
  `test` de chaque étape → rapport + journal série complet
  (`reports/<board>.{json,log}`) → `check`. Il vérifie que la puce
  annoncée = SoC du plan (mauvais port → arrêt). DTR/RTS relâchés avant
  l'ouverture (ponts auto-reset NodeMCU / ESP32-CAM-MB).
- **`firmware/hil/serial_tail.py`** : capture série sans reset, utilisée
  par l'E2E matériel (`e2e/tests/hw/pins.ts`) pour exiger
  `readback=HIGH/LOW`, l'absence de `MISMATCH` et la ligne `[IO]` de
  l'ADC souscrit, en plus de `last_value` côté API.

## 5. Garde obligatoire (CI)

`selftest::tests::every_board_has_a_passing_bench_report` :

1. toute carte avec `soc` a un rapport qui passe `check_report` ;
2. exception : `HIL_PENDING` (cartes antérieures à D122 non encore
   passées au banc) — liste qui **ne fait que décroître** (cliquet
   `HIL_PENDING_MAX`) ; une carte de la liste dont le rapport passe fait
   échouer le test tant qu'elle n'en est pas retirée ;
3. aucune nouvelle carte n'entre dans `HIL_PENDING`.

## 6. Premiers passages (2026-10-03)

| Carte | Résultat | Trouvé |
|---|---|---|
| `esp32-c3` (XIAO) | 47/47 | **Bug prod corrigé** : `analogWrite` du core prend un canal LEDC par pin et ne le rend jamais — le C3 (6 canaux) ne pilotait plus en PWM le 7ᵉ pin utilisé (5 étapes rouges au 1ᵉʳ passage). |
| `esp32cam-ai-thinker` | 9/9 | GPIO4 (LED flash) : pull-down de carte (grille du MOSFET), le pull-up interne lit 0 → flag profil `pull_down` (stocké en `mcu_boards.details`, omis si faux). |
| `esp32-devkit-38p-txd` | 90/90 | Rien : passe au 1ᵉʳ passage (TFT câblé). E2E `e2e:hardware` ajouté : wizard variante + écran TFT 1.77", build, flash, pins écran absents des pins pilotables, readback G25 + ADC G34. TFT validé à l'œil par l'utilisateur : panneau des pins live, G0 = 1 au repos (pull-up du bouton BOOT), 0 bouton appuyé ; entrées flottantes à 0. |
| `nodemcu_v3_oled` (CH340G, OLED soudé) | 27/27 | **Bug prod corrigé (ESP8266)** : les 6 étapes PWM rouges au 1ᵉʳ passage, le pad ne bougeait pas. `analogWrite()` du core 3.x ne fait `pinMode(OUTPUT)` que si son bit `analogMap` du pin est à 0, et ce bit survit à `pinMode(INPUT)` ; le `release` (`analogWrite(0)`) le positionnait → un pin relâché puis repassé en `pwm_out` restait en entrée. `release` passe par `digitalWrite` (arrête PWM/waveform, plus de pic LOW sur une entrée) et `pwm_out` force `OUTPUT`. Duty mesuré ensuite 0/50/100 exact. D8 (GPIO15) lit 0 en pull-up : pull-down de carte, strapping → niveau libre. E2E `e2e:hardware` (image reconstruite) : wizard variante (OLED imposé), build, flash prod, en ligne, readback D7, A0 souscrit : verts. |
| `esp32-c6-zero` (Waveshare, ESP32-C6FH8) | 66/66 | Nouvelle carte, **nouveau SoC** (`Soc::Esp32C6`) : le C6 exige le core Arduino 3.x (ESP-IDF 5), absent de la plateforme `espressif32` 7.x (core 2.0.17) → plateforme pioarduino épinglée (`55.03.312-1`) pour les seuls projets C6 (`generic_esp32c6`, env `esp32c6` du banc, prewarm de l'image builder). Lib : API LEDC 3.x indexée par pin (`ledcAttachChannel`/`ledcWrite(pin)`/`ledcDetach`) derrière une cale core 2/3, `WiFi.h` explicite pour l'OTA ; C3/ESP32/S3 recompilés sans régression. Bootloader @0x0 au merge (comme le C3), partitions `min_spiffs.csv` (l'app remplissait 96 % du slot 1,25 Mo). Vert au 1ᵉʳ passage : PWM 0/50/100, ADC1 GP0–GP5, TX/RX (16/17) libres (console USB native). E2E `e2e:hardware` : register/build/flash/en ligne, readback GP14 + ADC GP0, OTA, firmware IDE puis générique restauré : verts. |
| `esp32-devkit-38p-wroom32u` (WROOM-32U, antenne U.FL, CP2102) | 90/90 | Rien : passe au 1ᵉʳ passage (TFT 1.77" câblé, 2026-10-04). Même brochage que `esp32-devkit-38p-txd` ; seuls la sérigraphie (`15`, `D0`…`D3`, `RX`/`TX`) et le module (antenne externe : sans antenne sur l'U.FL, WiFi faible ou absent) diffèrent. |

Au passage : la lib ne compilait pas sur ESP32 sans OTA
(`esp_ota_ops.h` inclus seulement sous `PNEX_OTA_ENABLE`) — sketches
Tier 2 concernés, corrigé.

**E2E matériel (`task e2e:hardware`, image serveur reconstruite, 2026-10-03)** :
register/build/flash C3 + CAM, pins (readback API + série), télémétrie,
OTA, firmware custom : verts. Piège trouvé : pyserial relâche DTR avant
RTS → l'état DTR=0/RTS=1 **reset le C3** (USB-JTAG natif) et met EN à 0
sur les ponts auto-reset ; l'appareil repart, le test d'écriture voit
`last_value` absent (offline) et l'OTA suivante trouve l'appareil hors
ligne. Ouverture corrigée (RTS d'abord) dans `serial_tail.py` et le
harnais. La carte ESP32-CAM-MB se reset à l'ouverture quel que soit
l'ordre. Le test caméra (≥ 50 images / 20 s) a passé au 1ᵉʳ run puis
échoué aux suivants (6–15 images, RSSI −67 dBm) avec le même firmware :
liaison radio, pas ce chantier.

## 7. Definition of Done d'une nouvelle carte (complète brick0 §9)

1. Fichier `catalog/boards/<variante>.rs` + ligne `ALL` (D121).
2. `task fw:hil BOARD=<name> PORT=<port>` sur la carte réelle.
3. Rouge → corriger le firmware, ou le profil s'il décrit mal la carte
   (`pull_down`, pin réservé…) ; **jamais** les règles pour faire passer.
4. Rapport + journal commités ; `cargo test -p pnex-core selftest` vert.
5. E2E chaîne complète (`task e2e:hardware` si la carte y est déclarée).

## 8. Lots

| Lot | Contenu | Statut |
|---|---|---|
| T1 | `[IO]` + readback + `MISMATCH` (prod) | livré |
| T2 | Console `PNEX_TEST_MODE`, projet `firmware/selftest` | livré |
| T3 | Plan/verdict Rust + harnais `fw:hil` | livré |
| T4 | Garde CI + `HIL_PENDING` ; rapports C3 + CAM | livré |
| T4b | Rapports des cartes de `HIL_PENDING` (7 restantes au 2026-10-03, au fil du matériel disponible) | en cours |
| T5 | Gabarit de bouclage L2 (sortie → entrée, PWM → ADC via RC) | à faire |
