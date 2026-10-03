# Mode test des pins + banc HIL — exigence obligatoire par carte (D122)

> **Statut : EXIGENCE POSÉE (2026-10-02), non implémentée.** S'applique au
> firmware générique « type Firmata » (`firmware/lib/pnex`, Tier 1/2,
> `generic_esp8266` / `generic_esp32*`). Docs liés : `brick0.md` (§8
> safe-states, §9 DoD, D121 catalogue en code), `firmware-build.md`,
> `edge-model.md` (F2 StateReport, golden vectors).

## 0. Décision

| # | Décision | Motif |
|---|---|---|
| D122 | **Toute carte ajoutée au catalogue (`pnex_core::catalog::boards::ALL`) doit livrer un rapport de banc HIL vert** couvrant chaque pin GPIO de son profil dans chacun des modes que le SoC autorise (`digital_out`, `digital_in` ± pull-up, `pwm_out`, `adc_in`). Le rapport est produit par un **mode test firmware** qui journalise les **valeurs électriques réelles** (relues sur la broche), pas la valeur commandée. Garde CI bloquante côté `pnex-core`. | Aujourd'hui une carte entre au catalogue sur la foi de sa fiche de brochage : les invariants D121 vérifient la *cohérence* du profil (slots, GPIO connu du SoC) mais jamais qu'un `write` produit un niveau sur la broche, qu'une PWM sort réellement, qu'un ADC lit autre chose que du bruit. Les écarts découverts jusqu'ici (A0 lu en `digitalRead`, OLED V3 sur D5/D6 et non D1/D2, TX/RX provisionnés = série muette, I2C matériel inutilisable sur NodeMCU OLED) l'ont tous été à la main, sur carte, après coup. |

## 1. Constat (audit du 2026-10-02)

Ce qui existe :

- **Journal série des commandes** (`firmware/lib/pnex/src/pnex.cpp`) :
  `[PINS] …=GPIOn mode=… safe=…` à l'admission, `[CMD] set_mode`,
  `[CMD] write GPIOn -> HIGH|LOW|duty x%`, `[CMD] subscribe`. Ce sont les
  valeurs **commandées**, jamais relues.
- **`StateReport`** (`sendStateReport`) : `digitalRead` / `analogRead`
  réels pour les entrées, **mais rien n'est écrit sur la série** — la
  valeur ne se voit que côté serveur (`/pins` last_values, O2). Pour
  `pwm_out`, la valeur rapportée est `duty_pct` **mémorisé**, pas mesuré.
- **`core-cpp-tests`** : tests natifs hôte (golden vectors Rust = C++,
  ChaCha20) — aucun matériel, aucun GPIO.
- **Invariants catalogue** (`cargo test -p pnex-core catalog`) :
  cohérence statique du profil uniquement.
- **E2E matériel** : déroulés à la main (pyserial ad hoc, 2026-09-14
  NodeMCU, ESP32-C3, ESP32-CAM) ; aucun script versionné, aucun rapport
  conservé, rien de rejouable.

Ce qui manque : un mode test firmware, la journalisation des valeurs
réelles, un harnais hôte rejouable, et une garde qui rend tout cela
obligatoire.

## 2. Firmware — journalisation des valeurs réelles (toujours active)

Coût négligeable, utile en prod comme en debug :

1. **`StateReport` → ligne série** : `[IO] GPIOn mode=adc_in value=734`
   à chaque envoi (cadence de souscription inchangée).
2. **Relecture après `write`** : après `digitalWrite`, relire le niveau
   effectif de la broche et journaliser les deux :
   `[CMD] write GPIO5 -> HIGH (readback=HIGH)`. Un écart
   (`readback≠commande`) est journalisé `[IO] MISMATCH …` et remonte
   dans l'`Ack` (`ok=true` + champ `readback`) — le serveur n'est plus
   aveugle sur une sortie court-circuitée ou un pin input-only.
   - ESP8266 : `digitalRead` sur une sortie renvoie le niveau de la
     broche.
   - ESP32 : en `OUTPUT` simple le buffer d'entrée n'est pas activé —
     configurer `INPUT_OUTPUT` (`gpio_set_direction`) pour une vraie
     relecture, sinon on ne lit que le registre de sortie (à vérifier
     par SoC, consigné dans le rapport : `readback=latch|pad`).
3. **PWM** : rapporter le duty **effectivement programmé** (registre
   LEDC sur ESP32 — `ledcRead` / duty du canal ; sur 8266 la valeur
   passée à `analogWrite`) plutôt que `duty_pct` mémorisé.

## 3. Firmware — mode test (`PNEX_TEST_MODE=1`)

Build dédié (flag de compilation, **jamais** dans un artefact livré à un
device enregistré ; le pipeline serveur refuse le flag) :

- **Hors ligne** : pas de WiFi/WS, pas de config device requise — la
  carte est testable dès sa sortie du sachet.
- **Console série ligne à ligne** (115200), commandes Firmata-like :

  ```
  mode  <gpio> digital_out|digital_in|digital_in_pullup|pwm_out|adc_in
  write <gpio> 0|1
  pwm   <gpio> <duty 0..100>
  read  <gpio>
  info                      # SoC, fw, liste des GPIO compilés
  ```

- **Sortie machine** : chaque réponse = une ligne
  `PNEXT {"op":"read","gpio":4,"mode":"digital_in_pullup","value":1,"readback":"pad","t_ms":1234}`
  (préfixe fixe, JSON compact) — le harnais ignore tout le reste du
  journal. Erreurs : `PNEXT {"op":…,"err":"<code>"}` (codes machine,
  anglais).
- Les mêmes fonctions d'E/S que le chemin WS (`apply_pin`, écriture,
  lecture) : le mode test exerce **le code de prod**, pas une copie.
- Safe-state appliqué en entrée et en sortie du mode (même règle que
  brick0 §8).

## 4. Harnais hôte (`firmware/hil/`, Python via uv)

- Entrée : nom de carte du catalogue. Le profil (pins GPIO, labels, SoC)
  est exporté depuis `pnex_core::catalog` (petit binaire / test
  `--export`) — **jamais recopié à la main**.
- Pour chaque pin × mode autorisé par `caps::validate` pour ce SoC :
  - **Niveau L1 (sans câblage)** : `digital_out` 0/1 → relecture ;
    `digital_in_pullup` à vide → `1` ; `pwm_out` 0/50/100 → duty relu ;
    `adc_in` → valeur dans la plage du SoC, non figée sur 10 lectures.
  - **Niveau L2 (gabarit de bouclage)** : paires de pins câblées
    (sortie → entrée, PWM → ADC via RC) déclarées dans un fichier de
    câblage par carte ; vérifie les **valeurs électriques réelles** :
    `write 1` sur A ⇒ `read` B = 1, `pwm 50` ⇒ ADC ≈ mi-échelle (±
    tolérance), etc.
  - Ops interdites (strapping, flash GPIO6–11, input-only) : vérifie
    que le firmware les **refuse** proprement (pas de reset).
- Lecture série brute (pyserial) ; ouverture du port = reset DTR/RTS
  attendu et géré (attente de la bannière `PNEXT {"op":"ready"}`).
- **Jamais de flash implicite** : le port est toujours explicite
  (`--port`), pas d'auto-détection esptool (incident 2026-09-29).
- Sortie : rapport `firmware/hil/reports/<board>.json` — fw, SoC,
  date, niveau (L1/L2), et pour chaque pin × mode : commande, valeur
  relue, `pass|fail|skip(raison)`. Le journal série complet est joint
  (`<board>.log`).

## 5. Garde obligatoire (CI, bloquante)

Test `pnex-core` (à côté des invariants D121) :

1. Toute carte de `boards::ALL` avec `soc: Some(_)` a un rapport
   `firmware/hil/reports/<name>.json`.
2. L'ensemble des GPIO du rapport **= exactement** l'ensemble des pins
   `Gpio` du profil (un pin ajouté/renommé au profil sans re-test = rouge).
3. Chaque pin × mode autorisé est `pass`, ou `skip` avec une raison
   machine listée (ex. `strapping`, `flash`, `input_only`, `screen_bus`).
4. Niveau minimal **L1** ; **L2** requis pour qu'une carte soit marquée
   « validée matériel » dans l'UI (future colonne, hors périmètre ici).

**Transition** : les cartes déjà au catalogue sans rapport sont listées
dans une constante `HIL_PENDING` qui **ne peut que décroître** (le test
échoue si on y ajoute un nom). Aucune nouvelle carte n'y entre.

## 6. Definition of Done d'une nouvelle carte (complète brick0 §9)

1. Fichier `catalog/boards/<variante>.rs` + ligne `ALL` (D121).
2. Build `PNEX_TEST_MODE=1` flashé sur la carte réelle (port explicite).
3. Harnais L1 vert → rapport + journal commités.
4. L2 si le gabarit existe pour ce format de carte.
5. `cargo test -p pnex-core catalog` vert (garde §5).
6. E2E chaîne complète (WS → admission → write/subscribe UI) avec les
   valeurs `[IO]` du journal série cohérentes avec `/pins`.

## 7. Découpage d'implémentation

| Lot | Contenu |
|---|---|
| T1 | Journal `[IO]` des StateReports + readback après write + duty PWM réel (§2) |
| T2 | Mode test `PNEX_TEST_MODE` + console `PNEXT` (§3) ; refus du flag par le pipeline serveur |
| T3 | Export du profil depuis `pnex_core::catalog` + harnais `firmware/hil` L1 (§4) |
| T4 | Garde CI + `HIL_PENDING` (§5) ; rapports L1 des cartes physiquement disponibles (NodeMCU, NodeMCU V3 OLED, ESP32 38p TXD, XIAO C3, ESP32-CAM) |
| T5 | Gabarit L2 + fichiers de câblage |
