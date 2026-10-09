# Contrat WS d'ingestion capteurs — `/ws/sensor/ingest`

> Version officielle Rust (Phase 5). Parité du `SensorIngest` du POC
> initial avec les durcissements consignés ci-dessous.
> Ce fichier fait foi pour le client (firmware).

## 1. Connexion

```
GET wss://<hôte>/ws/sensor/ingest?device_id=<base64(device_id)>
Authorization: Bearer <base64(token)>
```

- **Jeton dans l'en-tête `Authorization`, jamais dans l'URL** (D154,
  lot L2 de `security-tiers.md` §6 bis) : une URL finit dans les journaux
  d'accès. Le paramètre `token` de l'URL est encore lu pendant la
  transition L2 (cartes flashées avant), puis supprimé.
- **TLS obligatoire** : le device passe par l'edge TLS, qui pose
  `X-Forwarded-Proto: https` et le secret de l'edge (`X-Pnex-Edge`) ;
  sinon close **4013** (« TLS required »). Désactivé seulement dans la
  config de test (`ingestion.require_tls`).
- **Certificat client** (D153) : le device se connecte au point d'entrée
  devices (`PNEX_DEVICE_PORT`, 4443 en LAN) avec le certificat émis par la
  CA de son org au build ; sans certificat valide **de ce device**, close
  **4014** (« Client certificate required »). Désactivé seulement en test
  (`ingestion.require_client_cert`).
- `token` et `device_id` sont **encodés base64 côté device** ; le serveur
  décode puis **trime** (les valeurs encodées à la `echo | base64` portent
  un `\n` final).
- Le token vient de la création du device (`POST /api/v1/devices` →
  `device_token.token`) ; il est unique par device et activable/désactivable.
- Pas de sous-protocole : l'upgrade est acceptée ou refusée sur l'en-tête
  et `device_id`, puis la poignée de main Noise (§2) ouvre le lien. Même
  règle pour `/ws/device`, `/ws/camera` et le téléchargement OTA
  (`GET /api/v1/ota/firmware/{device}/{version}?device_id=…` + en-tête).

## 2. Chiffrement : lien Noise (D156, remplace D8 le 2026-10-08)

`Noise_NNpsk0_25519_ChaChaPoly_SHA256`, clé partagée =
`device_token.encryption_key` (base64 de 32 octets, générée à
l'enregistrement), prologue `PNEX-NOISE-1|<device_id>`.

1. Première frame du device (texte) : base64 du premier message Noise
   (`-> psk, e`, 48 octets).
2. Réponse du serveur (texte) : base64 du second (`<- e, ee`, 48 octets).
   Poignée de main refusée (mauvaise clé, autre device, firmware antérieur)
   → close **4011**.
3. Ensuite, toutes les frames, **dans les deux sens**, sont des frames WS
   texte `base64(message de transport Noise)` : ChaCha20-Poly1305, nonce =
   compteur implicite par sens, tag de 16 octets.

- Base64 standard paddé des deux côtés.
- Frame falsifiée, modifiée, rejouée ou d'une autre connexion → réponse
  chiffrée `ERROR:decryption_failed` (le lien reste utilisable).
- Le format D8 (ChaCha20 sans Poly1305) n'existe plus (D157).

## 3. Device → serveur

| Frame (plaintext)                                     | Réponse (chiffrée)                  | Effet                                        |
|-------------------------------------------------------|-------------------------------------|----------------------------------------------|
| `PING` (casse ignorée)                                | `PONG`                              | heartbeat — rafraîchit le bail               |
| `name=value` (split 1er `=`)                          | `ok`                                | point de télémétrie horodaté serveur         |
| `ping=<x>`                                            | `ok`                                | mesure ordinaire + heartbeat (parité avec le POC initial) |
| sans `=`                                              | `error:invalid_format`              |                                              |
| `=v` (nom vide)                                       | `error:empty_key`                   |                                              |
| nom > 100 chars                                       | `error:measurement_name_too_long`   |                                              |
| mesure hors capacités (device strict)                 | `error:invalid_capability:<détail>` |                                              |
| nouvelle mesure au-delà du plafond (device dynamique) | `error:too_many_measurements`       |                                              |

- 1 mesure par frame, pas de JSON, pas de batch, pas de timestamp device
  (v1 ; D12 : provenance `ts_source` réservée pour la v2).
- **Normalisation du nom (D16)** : trim, accentspliés (`Température`→
  `temperature`), minuscules, tout non `[a-z0-9_:]` → `_` (répétitions
  fondues). `Soil-Moisture`, `soil moisture` et `soil_moisture` désignent
  la même mesure — la comparaison aux capacités, la découverte dynamique
  et le nom de série O2 utilisent toutes le nom canonique. Un nom qui
  normalise à vide (`---`) → `error:invalid_format`.
- Devices **stricts** (tous sauf les agents edge, D95) : le
  nom (normalisé) doit être une capacité du predefined device. Devices
  **dynamiques** : découverte automatique plafonnée à
  `max_unique_measurements` (100).

## 4. Close codes (frame Close après upgrade accepté)

| Code | Cause                                                         |
|------|---------------------------------------------------------------|
| 4001 | token inconnu/inactif, décodage impossible, erreur inattendue |
| 4002 | paramètre `token` absent                                      |
| 4003 | device déjà connecté (bail tenu — cf. §5)                     |
| 4005 | token invalidé en cours de session (revalidation ~10 s)       |
| 4006 | `device_id` ≠ device du token                                 |
| 4008 | clé de chiffrement absente/invalide                           |

## 5. Bail de vie / anti-clone (D9, décision user 2026-08-16)

L'identité (device_id + token) est bakée dans le firmware : deux devices
flashés du même build sont indistinguables. Le serveur applique un bail
**first-live-wins** :

- bail **Valkey** (D108) : clé à expiration (= TTL de silence) portant la
  session propriétaire, prise par un script Lua compare-and-set — une
  session vivante sur n'importe quel pod → 2e client rejeté **4003** ;
- bail expiré (crash, TCP à moitié ouvert, silence > TTL) → la reconnexion
  est admise, l'ancienne session est remplacée ;
- **déconnexion propre = bail libéré** (reconnect immédiat accepté),
  `device_states.last_seen_at` écrit en base ;
- bail et last_seen rafraîchis sur **toute frame valide** (throttle TTL/4,
  Valkey uniquement — aucune écriture Postgres) ;
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
