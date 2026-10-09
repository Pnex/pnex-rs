# OTA — mise à jour firmware par le réseau

> Statut : **implémenté (serveur + firmware + UI)** — la vérification
> matérielle reste à faire sur cartes réelles (checklists §7).
> Conception : `edge-model.md` §9 (réservations reprises ici).

## 1. Vue d'ensemble

```
USER (UI /devices)            BACKEND                                   DEVICE
1. « Mettre à jour (OTA) » ── POST /devices/{id}/ota {version?, force?}
                              gates : 409 ota_in_progress
                                      400 ota_not_ready (cap `ota` absente)
                                      409 same_version (force pour passer)
                                      400 no_ota_artifact / ota_too_large (8266)
                              ──► ota_assignments (pending = desired state)
                              ──► en ligne ? push OtaAvailable (WS) ────────► Ack{ok}
                              hors ligne → pushed:false, pickup à l'announce
2. DEVICE : GET {scheme}://{HOST}/api/v1/ota/firmware/{device}/{version}
            ?device_id= + Authorization: Bearer <token>   (posture /ws/device, D154)
            stream 1 KB → Update.h + sha256
            ◄── OtaState{downloading, pct} ≥5 s/5 % ──┤ (ESP32 : WS ouvert)
            sha256 vérifié AVANT Update.end (sinon abort, vieux fw conservé)
            signature Ed25519 vérifiée AVANT Update.end (SEC-18, §4)
            OtaState{flashing} → reboot
3. reboot → announce(fw=build #N) → serveur : fw ≥ cible → succeeded (+ journal
   in-app best-effort) ; announce fw < cible pendant downloading/flashing →
   failed (rollback visible) ; annonce tardive rattrape un failed(timeout).
4. watchdog serveur : downloading/flashing sans frame > 10 min → failed(timeout).
```

## 2. Machine à états

`pending → downloading → flashing → succeeded` ; `failed` possible depuis
tout état (Ack{ok:false}, sha mismatch, timeout 10 min, cancel UI, rollback
détecté à l'announce). `succeeded` est décidé **uniquement** par la
comparaison de version à l'announce post-reboot ; corrélation `cmd_id`
in-session (session unique par device, garde anti-clone) ; une annonce
tardive `fw ≥ cible` rattrape un `failed(timeout)`.

## 3. Version et artefacts

- **Version = id du build record** (monotone par device), injectée
  `-D PNEX_FW_VERSION` au build, annoncée par le device (`Announce.fw`),
  persistée côté serveur (`device_registries.fw_version`).
- Deux artefacts par build : l'image **mergée** (clé D6 historique, flash
  USB @0x0 — inchangée) et l'image **app brute** versionnée
  `org_{id}/ota/{device}/{version}.bin` + `ota_sha256` (payload OTA).

## 4. Sécurité

- Auth téléchargement (depuis 2026-10-09) = token brut en
  `Authorization: Bearer` + certificat client du device, en `https` sur le
  point d'entrée devices (même `authenticate_device` que `/ws/device`) ;
  le chemin doit nommer le device du jeton ; le sha256 annoncé dans `OtaAvailable`
  est recalculé côté device (mbedtls ESP32 / BearSSL 8266) et comparé
  avant le basculement de slot.
- TLS : point unique `pnex_tls` — `PNEX_CA_CERT` (b64 PEM) obligatoire
  (sans CA, aucune connexion : plus de `setInsecure()` depuis le
  2026-10-09). Depuis D70 la racine
  de l'edge est **injectée automatiquement** au build (cf. tls-edge.md) →
  WS et OTA vérifient ensemble ; renouvellement sous la même CA =
  transparent ; rotation de CA = un rebuild+OTA (ou USB).
- **Téléchargement différé** (2026-09-27) : `ota_available` n'enregistre
  que la demande (+ Ack) ; `loop()` lance le téléchargement une fois le
  callback WS dépilé. Avant : HTTPS + handshake mbedTLS imbriqués dans le
  handler de frame → `Stack canary watchpoint triggered (loopTask)` sur
  ESP32, reset muet (le HTTP clair passait). Le téléchargement suit le
  transport courant : device en `ws://` → `http://…:5150`, en `wss://` →
  `https://` via nginx.
- **Image signée (SEC-18, 2026-10-09)** : clé Ed25519 de l'instance,
  graine dans le coffre (secret plateforme `pnex-ota-signing-key`, créé
  au premier usage sous verrou de cluster, couvert par la rotation du
  keyring) ; clé publique compilée dans chaque build (`PNEX_OTA_PUBKEY`,
  hex) ; `OtaAvailable.sig` = signature de
  `"PNEX-OTA-1" ‖ len ‖ device_id ‖ len ‖ version ‖ sha256(image)`
  (`pnex_core::ota_sig`, miroir C++ `pnex_ota_sig.cpp` + Monocypher
  Ed25519, golden vectors `ota_sig_goldens.h`). Le device vérifie après
  le sha256, avant `Update.end` ; firmware sans clé ou ordre sans
  signature → refus avant tout téléchargement (« no ota key in
  firmware » / « unsigned image »), signature fausse → « bad
  signature ». Lier device et version empêche de rejouer l'image d'un
  autre device (qui porte ses identifiants) et de ré-étiqueter une vieille
  version. **Clé perdue = reflash USB de tous les devices** : jamais de
  rotation automatique. Un build qui ne peut pas lire la clé échoue
  (`build_ota_key`).
- **Anti-downgrade sur toutes les puces** (SEC-18) : version strictement
  inférieure refusée (égale admise : redéploiement forcé). Pas de version
  minimale en NVS : la version comparée est celle de l'image qui tourne,
  et la version cible est couverte par la signature. Garde serveur taille
  ≤ ~2 Mo (staging eboot 8266).
- Devices flashés avant SEC-18 : leur firmware ignore `sig` et accepte
  l'OTA (compat ascendante du champ) ; dès le premier build signé, seules
  les images signées passent. **Banc du 2026-10-09** (NodeMCU V3 OLED
  `proud-ibex`, ESP32-C6-Zero `noise-c6`, wss via l'edge) : OTA 19/20 →
  21/22 (bascule vers le firmware SEC-18), puis OTA signée 21/22 → 23/24
  vérifiée et acceptée sur les deux puces (pile 8266 tenue pendant la
  vérification Ed25519) ; OTA forcée vers 21/22 → « downgrade refused »
  sur les deux. Reste sur carte : signature invalide (couverte par les
  tests hôte `test_ota_signature_golden` ; le test matériel exige de
  remplacer la clé du coffre).

## 5. ESP8266 — modèle eboot (pas de migration)

Contrairement à l'ESP32 (slots app0/app1), l'OTA 8266 écrit la nouvelle
image dans l'espace libre **après** le sketch courant ; le bootloader
eboot la recopie sur 0x0 au reboot. Layout `eagle.flash.4m1m.ld`
**conservé** : aucune flash USB de migration. Contrainte :
`image actuelle + nouvelle image ≤ ~1 Mo` — app générique ≈ 441 Ko,
marge ≈ 600 Ko. `Update.begin` échoue proprement sinon (assignment
`failed`, vieux firmware conservé). Pendant le téléchargement, le WS est
fermé (RAM) : « fenêtre aveugle » sans progression live, résolue au
reboot par l'announce.

## 6. UI / notifications

- Page Devices : badge `build #N` (version courante annoncée), bouton
  « Mettre à jour (OTA) » (actif si cap `ota` + dernier build succès),
  modale de confirmation, progression live (poll 5 s de la liste, état
  hydraté dans `Device.ota`), notification in-app à l'issue (canal
  websocket de l'org, best-effort).
- Cas technicien / field kit : le même stack déployé sur le LAN
  (laptop/Pi = instance on-prem) suffit — les devices y pointent déjà,
  l'OTA passe en http brut local. Pas d'outil séparé.

## 7. Vérification matérielle (checklists)

**ESP32 (devkit / C3 / S3)**
0. ✅ **HTTPS validé en réel (2026-09-27, keen-badger / ESP32 devkit)** :
   CA locale épinglée, `https://192.168.1.185` via nginx, 1 015 328 o
   (~20 s) → sha OK → reboot → reconnexion wss. Bascule ws → wss par OTA
   validée aussi (téléchargement http :5150 depuis l'ancien firmware).
1. Flash USB (Web Serial) d'un build OTA ; device en ligne.
2. UI → « Mettre à jour (OTA) » → observer série : Ack → downloading %
   → flashing → restart → announce build #N+1 → UI « succeeded ».
3. Pins/souscriptions/régulations re-poussées à l'announce (desired state).
4. Coupure alim mi-download → vieux fw toujours booté ; assignment
   failed(timeout) au bout de 10 min ; retry OK.
5. Test négatif rollback : sans `esp_ota_mark_app_valid_cancel_rollback`
   la carte rebondit vers l'ancien slot au 1er reboot suivant.

**ESP8266 (NodeMCU)**
1. ✅ **Validé en réel (2026-09-22, eager-osprey / nodemcu_v3_oled)** : deux
   cycles complets `force` (build #14) — push → Ack → download → flash
   eboot → reboot → announce → succeeded (~23 s par cycle).
2. Variante wss (TLS) : vérifier la fenêtre aveugle (pas de progression
   pendant le download) et le heap (`[TLS]`/`[OTA]` logs).
3. Stress WDT (160 MHz, AP lent) : aucun reset pendant les écritures.
4. `Update.begin` échoue proprement si l'image dépasse le staging
   (`failed("begin: …")`).

## 8. Suivi

- Retention prune des artefacts `ota/` (garder N versions/device) —
  follow-up du watchdog.
- Progression 8266 en TLS : possible via reconnexion WS dédiée — non
  planifié.
