# Cartes de régulation mixtes (`pnex-reg-*`) — décision D20

> Phase 7. Source de vérité du contrat : `crates/pnex-core/src/proto.rs`
> (`ControlConfig`/`ControlSpec`/`RegState`) + `crates/pnex-core/src/flow.rs`
> (`RegTtConfig`/`RegPidConfig`). Math de référence : `pnex_core::control`.
> Décision : `docs/inventory.md` D20. Frontières : D13 (M2M edge), D17
> (downlink manuel), D18 (moteur ETL).

## 1. Le modèle produit

Une **carte de régulation mixte** est **un seul device** (board spécialisée,
firmware dédié — phase suivante) qui :

- lit **son propre capteur** (pin `digital_in`/`adc_in`) ;
- pilote **sa propre sortie** (pin `digital_out` — relais) ;
- embarque le régulateur : **tout-ou-rien chauffage**, **tout-ou-rien clim**
  (sens inversé) ou **PID** (sortie relais *time-proportional*).

L'ESP est **autonome** : il régule localement, même serveur coupé. Le
serveur ne fait que :

1. **authoring** — les cartes se posent dans le flow editor (palette +
   formulaires, validation live `validate_graph` en wasm) ;
2. **cast** — au deploy, la config est matérialisée en base
   (`regulator_configs`) puis poussée au device connecté ;
3. **re-config des seuils** — éditer une carte → redeploy → re-cast : le
   device applique sans reboot ;
4. **collecte** — la télémétrie standard (StateReport → O2) continue de
   remonter mesure + état de sortie.

**Anti-pattern banni** (D20) : récupérer une valeur d'une device pour
computer et l'appliquer sur une **autre** device — que ce soit par câblage
de flow (nœud device → calc → ?) ou par relais serveur. Le firmware
`4_chan_relay` (SensorData relayée par le serveur vers un actionneur
distinct) **est cet anti-pattern : déprécié, pilotage manuel uniquement**.
Une carte de régulation ne référence donc **jamais** deux devices — la
validation et le formulaire le rendent structurellement impossible.

## 2. Les trois cartes

| Carte (type Node-RED) | Kind | Sens | Paramètres spécifiques |
|---|---|---|---|
| `pnex-reg-tt-heat` | `tt_heat` | ON quand `mesure < consigne − deadband` | `deadband`, `min_on_secs`, `min_off_secs` |
| `pnex-reg-tt-cool` | `tt_cool` | ON quand `mesure > consigne + deadband` | idem |
| `pnex-reg-pid` | `pid` | duty % → relais cyclé | `kp`, `ki`, `kd`, `cycle_time_secs` |

Champs communs : `device_id` (slug), `sensor_pin` + `actuator_pin`
(**labels** d'authoring, résolus en gpio par le backend), `setpoint`,
`sample_ms` (200..=60 000), `data_timeout_secs` (capteur muet → safe),
`safe_state` (état de repos de la sortie).

## 3. Contrat fil — `ControlConfig` / `RegState`

**`ServerMsg::ControlConfig { cmd_id, configs: Vec<ControlSpec> }`** — RPC
acquitté (`Ack{cmd_id}`) :

- **Remplacement complet** : la liste portée remplace intégralement le jeu
  de régulations du device ; `configs: []` = clear (toutes les sorties
  régulées retournent au safe-state). C'est ce qui rend suppression et
  rollback gratuits (pas de message dédié).
- `ControlSpec` = struct **plat** (miroir ArduinoJson trivial) : champs PID
  à 0 pour les TT, `deadband` ignoré pour le PID. `node_id` = id canvas du
  nœud d'origine (clé du diag retour).
- Cap **4 configs par device** (buffer fil : `sendJson` 1024 o).
- Ordre garanti : **ProvisionAck → Subscribe (cadences) → ControlConfig**
  — l'apply du ProvisionAck réinitialise les pins côté firmware.

**Quand le serveur caste** :

1. **À l'announce** (après le ProvisionAck + re-push des Subscribe) :
   re-cast du desired-state — un reflash/reconnect retrouve ses régulations
   sans EEPROM ;
2. **À chaque deploy / rollback / suppression / set_mode du pin capteur**
   (points d'entrée uniques : `flows::reproject_and_cast[_extra]`) —
   indépendant du runtime ETL : jamais de 503 côté cast.

Device hors ligne au deploy → **rien** : le cast est différé à l'annonce
(même doctrine desired-state que les cadences Subscribe).

**`DeviceMsg::RegState { entries: Vec<RegDiag> }`** — diagnostic des
régulations (≤ 1 Hz), **toléré absent** (le firmware générique ne l'émet
jamais, il ignore poliment `ControlConfig` sans Ack) : `node_id`, `kind`,
`setpoint`, `sensor_value` (NaN si illisible), `output_pct` (0/100 TT, duty
PID), `cycle_count`.

## 4. Résolution des pins (le point délicat)

La table `regulator_configs` stocke l'**authoring** (labels), jamais les
gpio. À **chaque cast** le backend résout :

1. instances persistées (`device_capability_instances`, mode admis) —
   prioritaires ;
2. overlay board en complément (pin overlay-only = `digital_in`/`adc_in`,
   **jamais** une sortie : une sortie doit être admise) ;
3. validation au **point unique** `caps::validate` (sortie `digital_out`
   avec le `safe_state` de la carte) ;
4. jamais de `SetMode` automatique : un pin dans le mauvais mode → carte
   **sautée + warn** (insoluble), le device n'est pas modifié par surprise.

Conséquence voulue : repasser le pin actionneur en entrée rend la carte
insoluble → plus castée (le device retombe sur son safe au prochain
remplacement) ; repasser le pin **capteur** en sortie déclenche le
dé-déploiement automatique du flow (`stop_flows_reading_pin` étendu) +
cast `[]`.

**Unicité cross-flows** : `validate_graph` ne voit qu'un graphe — une
seule régulation par (device, sortie) est garantie au **sync** : plus
petit `flow_id` gagne, les suivantes sont sautées + warn (jamais de
blocage de deploy). Plafond : 4 cartes/device.

## 5. Le cycle de vie (sync/cast)

`services::regulator::sync_and_cast(db, extra_devices)` — re-projection
**complète** depuis la base, même doctrine que l'artefact `flows.json` :

```
flows deployed → graphes → regulator_configs_of
  → résolution device (slug+org → pk) / dédoublonnage sortie / plafond 4
  → réécriture intégrale de regulator_configs (transaction)
  → cast aux devices touchés CONNECTÉS (avant ∪ après ∪ extras)
```

Appelée par `flows::reproject_and_cast[_extra]` qui **précède** la
reprojection ETL : le sync/cast est **indépendant du runtime** (jamais de
503), un moteur coupé laisse base + devices cohérents.

⚠ Cas delete : la cascade FK efface les lignes de projection **avant** le
sync — le contrôleur capture donc les devices du flow **avant** delete et
les passe en `extra_devices` (sinon le clear `[]` ne part jamais ; test
`suppression_caste_liste_vide`).

## 6. Sémantique de régulation (contrat firmware)

Le firmware C++ de la carte mixte (phase suivante, `firmware/regulator`,
stack JSON+ChaCha20 de `generic_esp8266`) reproduit `pnex_core::control`
à l'identique. **Golden vectors livrés (2026-09-13)** : la référence Rust
génère les vecteurs (`crates/pnex-core/tests/golden_vectors.rs`), le
miroir `pnex-core-cpp` les rejoue sur l'hôte (`firmware/core-cpp-tests`,
Unity, CI firmware) :

- **TT** — hystérésis valeur **asymétrique** : ON au franchissement de
  `consigne ∓ deadband`, OFF au retour à la consigne ; + anti court-cycle
  temporel (`min_on_secs` ON-only, `min_off_secs`) ; `deadband ≤ 0` →
  jamais ON (fail-safe).
- **PID** — dérivée sur la **mesure** (`-kd·Δmesure/dt`, pas de derivative
  kick au changement de consigne, 1er échantillon D=0) ; anti-windup par
  **bornage de l'intégrale** (l'intégrale ne peut jamais pousser la sortie
  hors [0,100] au-delà de P+D) ; sortie duty 0..=100 % répartie sur le
  cycle relais (`relay_window` : ON sur les `duty%` initiaux du cycle).
- **Safe-states (décision control-cards)** : **WS down ≠ arrêt de
  régulation** — la carte mixte lit son capteur en local, elle continue de
  réguler hors ligne (c'est le produit). Sortie safe seulement : capteur
  illisible/hors plage > `data_timeout_secs`, ou boot sans config. Au
  retour du lien, le serveur re-caste → re-synchro. StateReports throttlés
  lien down.

**Optimisation des gains PID via l'historique OpenObserve** (évoqué en
étude) : hors périmètre immédiat — l'architecture y prépare (mesure +
`output_pct` remontent en série O2, un tuner offline peut rejouer).

## 7. Les nœuds runtime sont passifs (pourquoi)

Sans crate liée, un type inconnu du moteur est dégradé en nœud `unknown`
(warn seul, no-op silencieux). `crates/pnex-node-control` enregistre les
trois types pour rétablir le **fail-loud au build** (`BadFlowsJson` →
`redeploy_failed`) et valide via la **vraie** `validate_graph` (une seule
source de vérité backend/wasm/runtime). Le nœud ne régule pas — sa boucle
consomme et jette (anti-engorgement si un producteur est câblé).

## 8. Limites assumées (phase 7)

- Firmware de la carte mixte **non écrit** — le contrat ci-dessus est
  testé côté serveur par un device WS mocké (`tests/regulator.rs`) ;
- `RegState` routé vers O2 depuis le 2026-09-13 (F2, edge-model.md §11) :
  séries `<node>_sensor/_out/_cycles`, `source_type=reg_state` — mesure NaN
  = pas de point (trou, jamais d'invention) ;
- conflit cross-flows = warn + skip (pas de blocage du deploy) ;
- remplacement de config pendant un cycle PID = safe-first (reprise au
  prochain échantillon) — l'init bumpless de l'intégrale reste à faire ;
- le formulaire n'expose qu'**un** select device (la contrainte « carte
  mixte » est structurelle).
