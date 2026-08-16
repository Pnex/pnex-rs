# Contrat WS d'ingestion capteurs — `/ws/sensor/ingest`

> Version officielle Rust (Phase 5). Parité du `SensorIngest` Django POC
> (`docs/phase0/ws-channels-crypto.md` §2.1) avec les durcissements
> consignés ci-dessous. Ce fichier fait foi pour le client (firmware).

## 1. Connexion

```
GET ws(s)://<hôte>/ws/sensor/ingest?token=<base64(token)>&device_id=<base64(device_id)>
```

- `token` et `device_id` sont **encodés base64 côté device** ; le serveur
  décode puis **trime** (les valeurs encodées à la `echo | base64` portent
  un `\n` final).
- Le token vient de la création du device (`POST /api/v1/devices` →
  `device_token.token`) ; il est unique par device et activable/désactivable.
- Pas de headers d'auth, pas de sous-protocole, pas de message
  d'handshake : l'upgrade est acceptée/refusée sur la query string.

## 2. Chiffrement (D8 : ChaCha20 nu, pas d'AEAD)

Toutes les frames, **dans les deux sens**, sont des frames WS **texte** :

```
base64( nonce(12 octets) ‖ ChaCha20(ciphertext) )
```

- Clé : `device_token.encryption_key` (base64 de 32 octets, générée à
  l'enregistrement).
- Nonce : 12 octets aléatoires **frais à chaque message** (RFC 7539).
- Base64 standard paddé des deux côtés.
- Pas de Poly1305 (parité Django pycryptodome ; montée AEAD versionnée à
  venir). Frame illisible → réponse chiffrée `ERROR:decryption_failed`.

## 3. Device → serveur

| Frame (plaintext) | Réponse (chiffrée) | Effet |
|---|---|---|
| `PING` (casse ignorée) | `PONG` | heartbeat — rafraîchit le bail |
| `name=value` (split 1er `=`) | `ok` | point de télémétrie horodaté serveur |
| `ping=<x>` | `ok` | mesure ordinaire + heartbeat (parité Django) |
| sans `=` | `error:invalid_format` | |
| `=v` (nom vide) | `error:empty_key` | |
| nom > 100 chars | `error:measurement_name_too_long` | |
| mesure hors capacités (device strict) | `error:invalid_capability:<détail>` | |
| nouvelle mesure au-delà du plafond (device dynamique) | `error:too_many_measurements` | |

- 1 mesure par frame, pas de JSON, pas de batch, pas de timestamp device
  (v1 ; D12 : provenance `ts_source` réservée pour la v2).
- **Normalisation du nom (D16)** : trim, accentspliés (`Température`→
  `temperature`), minuscules, tout non `[a-z0-9_:]` → `_` (répétitions
  fondues). `Soil-Moisture`, `soil moisture` et `soil_moisture` désignent
  la même mesure — la comparaison aux capacités, la découverte dynamique
  et le nom de série O2 utilisent toutes le nom canonique. Un nom qui
  normalise à vide (`---`) → `error:invalid_format`.
- Devices **stricts** (modèle `custom_sensor`/`custom_device` exclus) : le
  nom (normalisé) doit être une capacité du predefined device. Devices
  **dynamiques** : découverte automatique plafonnée à
  `max_unique_measurements` (100).

## 4. Close codes (frame Close après upgrade accepté)

| Code | Cause |
|---|---|
| 4001 | token inconnu/inactif, décodage impossible, erreur inattendue |
| 4002 | paramètre `token` absent |
| 4003 | device déjà connecté (bail tenu — cf. §5) |
| 4005 | token invalidé en cours de session (revalidation ~10 s) |
| 4006 | `device_id` ≠ device du token |
| 4008 | clé de chiffrement absente/invalide |

## 5. Bail de vie / anti-clone (D9, décision user 2026-08-16)

L'identité (device_id + token) est bakée dans le firmware : deux devices
flashés du même build sont indistinguables. Le serveur applique un bail
**first-live-wins** :

- session ouverte en-process → un 2e client est rejeté **4003**
  immédiatement ;
- fallback base : `device_states.last_seen_at` frais (< TTL) d'une session
  non refermée (crash, autre process) → 4003 ;
- **déconnexion propre = bail libéré** (reconnect immédiat accepté) ;
- last_seen rafraîchi sur **toute frame valide** (throttle ~1 s d'écriture) ;
- reaper (5 s) : `active=true` si frais, `false` si silence > TTL
  (défaut **10 s** = 2 PING manqués à 5 s ; `PNEX_SILENCE_TTL_SECS`).

Limite assumée : après TTL de silence, un clone **peut** prendre la place
(pas d'identité physique sans provisioning par device).

## 6. Sortie des données (D1/D2)

Backend → **metrics OpenObserve** de l'org du device (org provisionnée
automatiquement à la première donnée) :

```
POST /api/{o2_org}/prometheus/api/v1/write   # WriteRequest protobuf, snappy
```

Séries : `<metric_name>{device_id, pred_dev, source_type="sensor", ts_source="server"}`
— nom de métrique assaini (`[a-zA-Z_:][a-zA-Z0-9_:]*`). Batch 500/10 s.
Valeurs non numériques écartées. Requêtable via
`/api/{o2_org}/prometheus/api/v1/query`.

## 7. Client de référence

`cargo run -p pnex-backend --example ingest_client -- --url … --token …
--device-id … --key … [--hold]` (mimique firmware : PING + key=value
chiffrés, affiche les close codes).
