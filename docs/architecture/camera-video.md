# Caméra & vidéo — collecte, live, enregistrement, vision, événements (D73–D86)

> **Statut (2026-09-29) :** phases V et W **implémentées** (firmware
> compilé non flashé, backend, runtime, UI) ; **E2E matériel ESP32-CAM à
> faire** à réception de la carte ; D86 **livré** le 2026-09-29
> (`notifications.md` §15). Voir §8.
> **Déclencheur :** ESP32-CAM commandé — première source vidéo réelle de
> l'axe C (`roadmap.md`), tranche le point « sources vidéo V1 » de
> `ml-vision.md`.
> Renvois : `ml-vision.md` (pilier, stack tout-Rust), `media.md` (D21,
> MediaStore fs/S3), `flow-engine.md`, `notifications.md`,
> `firmware-build.md`, `edge-model.md`.

## 1. Le besoin

1. **Collecte vidéo** depuis un ESP32-CAM (puis toute caméra) ;
2. **Live** sans stockage, **sans cron** (« every 1 s ») : poussé à
   l'image près, event-driven de bout en bout ;
3. **Dans le flow** : choisir de stocker ou non (S3/RustFS ou disque),
   avec les paramètres de découpage (durée de segment, taille max, fps,
   rétention) ;
4. **Phase 2** : modèles pré-entraînés (objets / humains) chargés dans un
   **registre** (CRUD + test sur une image), branchables dans le flow ;
5. **Événements texte** (JSON logs) stockés hors base relationnelle —
   OpenObserve (logs), pas Postgres/sqlite ;
6. (Roadmap seulement) journal des notifications → O2 + onglet
   « Événements » dans /notifications.

## 2. Contraintes constatées dans le code (2026-09-29)

- Le canal device `/ws/device` transporte du **JSON chiffré en texte
  base64**, `MAX_PLAIN` 4096 (ESP32) — une frame JPEG (10–60 Ko) n'y
  passe pas, et le serveur ignore les frames WS binaires.
- Le moteur de flows (edgelink) véhicule des messages **JSON** : y faire
  transiter des octets JPEG (base64) multiplierait le coût par 1,33 et
  par le nombre de branches (deep_clone par fan-out).
- `MediaStore` (opendal fs/s3) écrit des **buffers entiers** — adapté à
  des segments de quelques Mo, pas à un flux continu.
- Aucun nœud **source** événementiel n'existe (tout part d'un `inject`) ;
  aucune ingestion **logs** O2 (`_json`) ni `_search` n'existe.
- Valkey est déjà là (cache live D-b966c1a), `VALKEY_URL` déjà injecté
  dans le runtime.

## 3. Architecture

```
ESP32-CAM ──/ws/device (JSON chiffré, contrôle)──────────► backend
    │        ◄── ServerMsg::CameraConfig (fps, résolution, on/off)
    │
    └──/ws/camera (binaire : nonce‖ChaCha20(header‖JPEG))──► CameraHub (backend)
                                                              │  ├─ dernière frame + broadcast
                                                              │  │     └─► /ws/camera/live (navigateur) — LIVE sans stockage
                                                              │  └─ Valkey : SET frame (TTL 15 s) + PUBLISH meta
                                                              ▼
                                   flow runtime : [camera-source] ──► [video-record] ──► /internal/flow/video-segment
                                   (SUBSCRIBE, event-driven)     └──► [vision-detect] (phase W) ──► [event-log] ──► O2 logs
```

### D73 — Canal vidéo dédié `/ws/camera`, binaire, même clé device

Deuxième WebSocket du device, **uplink seulement**, frames **binaires** :
`nonce(12) ‖ ChaCha20(clé device, header ‖ JPEG)` — même posture que D8
(ChaCha20 RFC 7539 nu, nonce frais par frame), sans base64. Auth
identique à `/ws/device` (`?token=&device_id=` base64, `Snapshot::load`),
anti-clone par un registre `CAMERA_SESSIONS` séparé (close 4003).
Le contrôle (config caméra, acks, OTA, pins) reste sur `/ws/device`.

Header clair (16 octets, little-endian) :

| Octets | Champ | Note |
|---|---|---|
| 0..4 | magic `PXC1` | version du format |
| 4..8 | `seq` u32 | incrémental, wrap libre |
| 8..12 | `uptime_ms` u32 | horloge device (info) |
| 12..14 | `width` u16 | |
| 14..16 | `height` u16 | |

Puis le JPEG (SOI `FF D8` vérifié). Plafond serveur : 512 Ko/frame
(`PNEX_CAMERA_MAX_FRAME_BYTES`), fps plafonné côté serveur au fps
configuré + 50 % (frames excédentaires jetées, compteur). Le timestamp
de référence est **celui du serveur** (`ts_source=server`).

*Rejeté* : MJPEG pull (le device sert `/stream`) — casse le modèle
serveur-centrique (NAT, device jamais joignable) ; HTTP POST par frame —
une requête TLS par frame, trop cher sur ESP32 ; RTSP sur ESP32 —
immature et pull.

### D74 — CameraHub : live en mémoire, bus Valkey pour les flows

`services/camera.rs` : par device, dernière frame (`Arc<Frame>`) +
`tokio::broadcast` (capacité 4, les viewers lents perdent des frames,
jamais de backpressure sur l'ingest). Quand Valkey est configuré, chaque
frame est aussi publiée pour le moteur de flows :

- `SET pnex:cam:v1:{org}:{device}:f:{seq} <jpeg> EX 15`
- `PUBLISH pnex:cam:v1:{org}:{device} {"seq","ts_ms","w","h","size"}`

Les messages de flow ne portent **qu'une référence** (clé Valkey) — les
octets ne traversent jamais le JSON du moteur (§2). Sans Valkey : live OK,
nœuds caméra en erreur explicite (`camera_bus_unavailable`), jamais de
panique.

### D75 — Live navigateur `/ws/camera/live` (sans stockage, sans cron)

`GET /ws/camera/live?token=<JWT>&org=<id>&device=<pk>` (auth école
`/ws/notify`) : à la connexion, envoi immédiat de la dernière frame, puis
chaque frame du broadcast en **binaire JPEG brut** (TLS de l'edge pour le
chiffrement). Côté front : module JS `pnexViewers.camera` (pattern
viewers — marche web + webview Android via le pont eval) qui pose
chaque frame en blob URL sur un `<img>`. Aucun polling.

### D76 — Demande de capture : `continuous` ou `on_demand`

Table `device_cameras` (1 ligne/device, créée à l'announce du cap
`camera`) : `framesize` (QVGA/VGA/SVGA/XGA/HD/SXGA/UXGA), `quality`
(10–63, JPEG ESP), `fps` (1–25), `capture_mode`, `vflip`, `hmirror`.

- `continuous` : le device pousse tant qu'il est connecté (requis dès
  qu'un flow enregistre ou détecte en continu) ;
- `on_demand` (défaut) : le serveur envoie `CameraConfig{enabled:true}`
  au premier viewer live, `enabled:false` 30 s après le dernier départ.

`ServerMsg::CameraConfig{cmd_id, enabled, framesize, quality, fps,
vflip, hmirror}` est poussé après l'announce, à chaque PATCH des
réglages et à chaque transition de demande ; le device répond `Ack`.

### D77 — Firmware : projet `generic_esp32cam` + module caméra de la lib

- Carte de référence : **AI-Thinker ESP32-CAM** (OV2640, PSRAM 4 Mo),
  `pio_board = esp32cam`, partitions `min_spiffs.csv` (2 slots OTA de
  1,9 Mo — `default.csv` et ses 1,3 Mo sont trop justes avec caméra +
  TLS + écrans).
- Lib PneX : `pnex_camera.{h,cpp}` (compilé seulement si
  `PNEX_CAMERA_ENABLE=1` et ESP32) — `esp_camera` du core Arduino,
  second `WebsocketsClient`, buffer chiffré en PSRAM, pacing au fps
  configuré ; `PnexDevice` gagne deux points d'extension génériques :
  caps additionnelles (`camera/video`) et hook des messages serveur
  inconnus (le module caméra consomme `camera_config`).
- Pins AI-Thinker réservées dans le profil board ; exposées au
  provisioning : **GPIO4** (LED flash, sortie) et **GPIO33** (LED rouge,
  sortie inversée). GPIO 12/13/14/15/2 (bus SD) : libres si pas de carte
  SD, non exposés en V1 (GPIO12 = strapping VDD_SDIO).
- Build serveur inchangé (predefined `generic_esp32cam` = dossier du
  projet), flash via le bouton existant (FTDI + GPIO0 à la masse : le
  AI-Thinker n'a pas d'USB — le carrier « ESP32-CAM-MB » en a un).

### D78 — Enregistrement dans le flow : segments MJPEG-AVI

- Nœud **`camera-source`** (source événementielle, premier du genre) :
  `SUBSCRIBE` au canal du device, émet un message par frame
  `{device, seq, ts_ms, width, height, size, frame_key}` ; option
  `max_fps` (échantillonnage : la vision peut tourner à 1 fps pendant
  que l'enregistrement prend tout).
- Nœud **`video-record`** : accumule les frames (GET par `frame_key`)
  dans un segment **AVI MJPEG** (conteneur trivial à écrire en Rust pur,
  lisible par VLC/ffmpeg, pas de transcodage, pas de dépendance native)
  et le **flush** quand `segment_secs` (défaut 60) ou `max_segment_mb`
  (défaut 32) est atteint, ou après `gap_secs` sans frame (défaut 10).
  `max_fps` propre au nœud, `retention_days` (0 = illimité),
  `stream` (nom logique, défaut = nom du device).
- Flush = `POST /internal/flow/video-segment` (jeton de service flow,
  école `device-write`) : le **backend** écrit le blob via `MediaStore`
  (fs ou S3/RustFS selon la config plateforme — même sélecteur que le
  média) puis la ligne `video_segments`. Le runtime ne détient aucun
  secret de stockage.
- Sortie du nœud : un message par segment écrit (`segment_id`, durée,
  frames, octets) — chaînable (notification « segment écrit », etc.).
- **Pas de nœud record = pas de stockage** : le live et la vision
  fonctionnent sans rien écrire (réponse directe au besoin « storage ou
  pas »).

Clé de stockage : `org_{org}/video/{device}/{yyyy}/{mm}/{dd}/{segment}.avi`.

### D79 — Table `video_segments` + rétention

`video_segments` (UUID, org-scoped) : `device_registry_id` (FK CASCADE),
`flow_id` (nullable, SET NULL), `node_id`, `stream`, `started_at`,
`ended_at`, `frame_count`, `size_bytes`, `width`, `height`,
`storage_key`, `expires_at` (nullable). Index `(org_id,
device_registry_id, started_at DESC)`. Pruner horaire (école
`notify_deliveries`) : supprime blob puis ligne pour `expires_at < now`.
Les octets ne sont **jamais** en base.

### D80 — UI : page `/cameras`

Liste des caméras (devices porteurs du cap), carte = live (lecture à la
demande), réglages (D76), onglet **Enregistrements** (segments paginés
D14, filtre caméra/jour) avec lecteur intégré (AVI parsé côté front par
`pnex_core::video::avi`, frames en blob URL au fps enregistré) et
téléchargement du `.avi`.

## 4. Phase W — vision et événements (spécifiée, à implémenter ensuite)

### D81 — Registre de modèles = média `kind=model` + spec d'inférence

Les octets (ONNX) vivent dans la bibliothèque média (versioning,
stockage fs/S3 et purge **déjà** livrés — D21), nouveau kind `model`
(sniff `.onnx`). Table `ml_models` : `asset_id` (+ version épinglée
optionnelle), `task` (`detection` V1, `classification` ensuite),
`family` (`yolox`, `ssd`, …) qui fixe le pré/post-traitement,
`input_width/height`, `labels` (JSON, COCO par défaut), `score_threshold`,
`nms_iou`. Modèles de référence **Apache-2.0** : YOLOX-nano/tiny (jamais
YOLOv8/v11 AGPL). CRUD + `POST /api/v1/ml/models/{id}/test` (image
octet-stream → détections + temps d'inférence), UI « Tester avec une
image » qui dessine les boxes.

### D82 — Inférence : crate `pnex-vision` (tract), serveur d'abord

Crate pure Rust `pnex-vision` (tract-onnx + décodage JPEG `image`) :
utilisée par l'endpoint de test **et** par le nœud flow. Cible V1 =
runtime de flows sur le serveur ; l'edge (mini-PC) et le GPU passent par
la fabric de workers (P2.7) plus tard. Burn reste le challenger du POC
(`ml-vision.md` étape 1).

### D83 — Nœud `vision-detect`

Entrée : message `camera-source` (ou image média) ; charge le modèle au
deploy (octets tirés par `/internal/flow/ml-model/{id}`, cache disque du
runtime par sha256) ; sortie `payload.detections[{label, score, bbox}]`,
filtres `labels` / `min_score`, mode `emit: always | on_detection`,
option `snapshot` (la frame annotée est écrite en média — preuve
visuelle d'un événement).

### D84 — Événements JSON → OpenObserve logs, jamais la base

Client O2 : `ingest_json(org, stream, docs)` (`POST
/api/{o2_org}/{stream}/_json`) + `search(org, sql, range)` (`_search`).
Nœud **`event-log`** : écrit `payload` (objet JSON) + contexte
(`flow`, `node`, `device`, `level`, `ts`) dans le stream `ev_<nom>`
(défaut `ev_events`) de l'org. Page **Événements** (recherche plein
texte + filtres, période) via un endpoint backend `_search` scoppé org.
Rétention = celle d'O2 (réglable par stream, D72).

### D85 — Détections → événements → notifications

Chaîne type : `camera-source (1 fps) → vision-detect (person, ≥0.6,
on_detection) → event-log + notify`. L'anti-spam des notifications
(N3) s'applique ; l'événement garde le lien vers le snapshot.

### D86 — Journal des notifications → O2 (livré 2026-09-29)

`notify_deliveries` (Postgres, pruning 30 j) migre vers un stream O2
`notify_deliveries` (**tous** les envois y compris webhooks du nœud flow,
avec le statut retour HTTP) ; onglet **Événements** dans /notifications
(canaux · modèles · événements). Détail et écarts : `notifications.md`
§15.

## 4 bis. Phase W5 — fiabilité vision (D100–D104, 2026-09-30)

Constat du premier E2E réel ESP32-CAM : **trois échecs silencieux**
enchaînés, aucun message nulle part.

1. `yolox_l.onnx` enregistré en 416×416 (défaut) alors que le fichier
   impose 640×640 → `Detector::load` échoue (« Failed analyse … Slice_4 »),
   le nœud boucle en retry, frames jetées, erreur perdue dans des logs
   runtime filtrés en production.
2. Même piège avec `yolox_nano` réglé à 640 (il impose 416).
3. `score_threshold` du modèle (0,35) filtre la personne vue à 0,25 sans
   aucune trace ; et en `on_demand` (défaut D76) un flow seul ne réveille
   pas la caméra — il ne reçoit des frames que si quelqu'un regarde le live.

Principe : **l'utilisateur ne saisit jamais ce que le fichier dicte, et
toute image jetée est comptée et expliquée.**

### D100 — Introspection et validation du modèle à l'enregistrement

- `pnex-vision::inspect(onnx)` lit l'entrée déclarée (dimensions fixes ou
  libres) et la forme de sortie (nombre de classes pour YOLOX :
  `C − 5`).
- `pnex-vision::validate(onnx, spec)` charge le modèle **et** exécute une
  inférence sur une image neutre à la taille d'entrée → temps de
  chargement + temps d'inférence mesurés sur **ce** serveur.
- Création / modification : dimensions fixes du fichier → **imposées**
  (la saisie est ignorée, le champ est verrouillé dans l'UI avec « lu dans
  le fichier ») ; nombre de labels ≠ classes de sortie → erreur de champ
  `labels` (`count:<n>`) ; échec de chargement/inférence → 422
  `ml-model-load-failed` avec le diagnostic tract (exception verbatim
  « diagnostic runtime »).
- `ml_models` gagne `check_status` (`unchecked` | `valid` | `invalid`),
  `check_error`, `infer_ms`, `checked_at` (migration 000041). Les modèles
  existants restent `unchecked` ; bouton **Vérifier**
  (`POST /api/v1/ml/models/{id}/check`) qui réécrit le statut.
- Endpoint `GET /api/v1/ml/inspect?asset_id=` : pré-remplissage du
  formulaire dès le choix du fichier.

### D101 — Limites affichées et garde de déploiement

- `/models` affiche statut, temps d'inférence mesuré et le **débit
  soutenable** (`≈ 1000 / infer_ms` images/s) ; avertissement si le
  `max_fps` d'un nœud dépasse ce débit.
- Déploiement refusé si un `vision-detect` référence un modèle
  `invalid` ou supprimé (violation de flow `vision-model-invalid` +
  args `{model}`) — même esprit que le trigger booléen des notifications
  (D66).

### D102 — Un flow déployé réveille la caméra

La demande de capture (D76) devient `continuous ∨ viewers > 0 ∨ flow` :
les devices référencés par un `camera-source` d'un flow **déployé** gardent
l'uplink ouvert (recalcul au boot, à chaque deploy/stop/suppression).
`on_demand` ne concerne plus que le live navigateur.

### D103 — Diagnostics des nœuds caméra/vision

- `camera-source` et `vision-detect` publient un **statut** sur le canal
  debug du moteur (format `pnex-status`, id canvas brut comme
  `pnex-display`) : `{level, code, detail, stats}` aux transitions
  (modèle en chargement / prêt / erreur, bus indisponible, aucune frame
  depuis N s) + un battement toutes les 10 s avec les compteurs
  (`received`, `stale`, `throttled`, `analysed`, `emitted`,
  `below_threshold`, `last_infer_ms`).
- Codes machine (`vision-model-loading`, `vision-model-ready`,
  `vision-model-load-failed`, `camera-no-frames`…) résolus par le front
  (clés `flow-status-*`), détail verbatim en diagnostic.
- Éditeur : badge d'état sous le nœud (vert / ambre / rouge + compteurs) ;
  entrées « état » dans le drawer debug.
- Les statuts sont de la **santé**, pas du debug : dernier statut par
  nœud gardé à part du feed (`GET /api/v1/flows/{id}/node-status`, TTL
  60 s, purgé au deploy), **visible en mode run** (`debug_tools` off) —
  le feed debug, lui, reste réservé au mode dev.

### D104 — Test live sur `/models`

- `POST /api/v1/ml/models/{id}/test-live?device=<id>` : rejoint la
  demande de la caméra le temps de l'appel (la grâce de 30 s de D76
  absorbe l'enchaînement des appels), prend la **dernière frame** du
  CameraHub, exécute le modèle avec un plancher de score bas (0,05) et
  renvoie : frame JPEG (base64), détections **toutes** (y compris sous le
  seuil), seuil du modèle, âge de la frame, état caméra
  (connectée, mode, résolution, fps) et avertissements machine
  (`camera-offline`, `camera-no-frame`, `camera-frame-stale`,
  `image-too-dark`, `inference-slower-than-camera`).
- Modal « Tester en live » : sélection de caméra, image + boxes (au-dessus
  du seuil : pleines ; en dessous : pointillées grises), liste des scores,
  curseur de seuil local + « Appliquer ce seuil au modèle », boucle
  requête-après-réponse (≥ 500 ms), arrêt à la fermeture.

### Ordre d'implémentation

1. `pnex-vision` : `inspect`, `validate`, `detect_with_floor`, luminance.
2. Migration 000041 + service + endpoints `inspect`/`check`/`test-live`
   + codes d'erreur et clés i18n.
3. Demande caméra par les flows (D102).
4. Statuts des nœuds (D103) : runtime + feed + badges éditeur.
5. UI `/models` : statut, limites, formulaire verrouillé, modal live.
6. Garde de déploiement (D101).
7. Tests (unitaires + intégration backend), image, E2E réel ESP32-CAM.

## 4 ter. Enregistrement continu + calques de détection (D105, 2026-09-30)

### D105 — 1 caméra = 1 enregistrement, N calques de détection dans O2

- **Un seul `video-record` par caméra** dans l'org : violation
  `video_record_duplicate` à la sauvegarde (deux enregistreurs dans le même
  flow) et porte de déploiement `camera-already-recorded` entre flows (même
  école que `pin-already-assigned` : le redeploy du propriétaire passe). La
  caméra d'un enregistreur = les `camera-source` en amont, par n'importe
  quel chemin (`video_record_claims_of`). Constat à l'origine : deux nœuds
  `video-record` du même flow écrivaient les mêmes images en double.
- **Les segments restent petits sur disque** (60 s par défaut : résilience
  au crash, rétention à la minute) — pas de compaction. L'API les présente
  comme un enregistrement continu : timeline du jour
  (`GET /cameras/{device}/recordings`), export d'une plage en un seul AVI
  (`…/recordings/export`, 256 Mo de source max, segments chevauchants
  sautés par `playback_chain`), suppression d'une plage (owner/admin).
- **Calques de détection = cadres seuls, jamais de pixels** : un nœud
  `vision-detect` avec `record_layer` pousse ses détections (bufferisées
  5 s, par caméra) vers `/internal/flow/video-annotations` → stream O2
  `camera_detections`, `_timestamp` = heure de la frame, `layer_id =
  f{flow}-{node}`, nom = nom du nœud sinon nom du modèle. Nombre de calques
  illimité, depuis n'importe quel flow, sans câblage vers l'enregistreur.
  Quelques centaines d'octets par frame analysée contre une vidéo annotée.
- **UI `/cameras`** = table CRUD (socle) ; actions Direct / Enregistrements
  / Réglages en modales. La fenêtre Enregistrements : jour, timeline,
  lecteur enchaîné (préchargement du segment suivant, ×1–×8), cases à
  cocher des calques du jour → surimpression SVG (tolérance ±750 ms, une
  couleur par calque).
- Reste : la rétention du stream `camera_detections` suit la rétention O2
  de l'org (D72), pas celle des segments.

## 5. Phases

| Phase | Contenu | Critère de sortie |
|---|---|---|
| **V1** | Proto (`CameraConfig`, header `PXC1`, cap `camera`), `/ws/camera`, CameraHub, `/ws/camera/live`, `device_cameras` + API réglages | Frames d'un faux device (test d'intégration) visibles en live |
| **V2** | Firmware `generic_esp32cam` + board/predefined + seed | ESP32-CAM réel : live dans /cameras |
| **V3** | Nœuds `camera-source` + `video-record`, `video_segments`, endpoint interne, pruner | Segments `.avi` écrits (fs et S3/RustFS), lisibles VLC |
| **V4** | Page `/cameras` (live, réglages, enregistrements + lecteur) | E2E UI headless + réel |
| **W1** | Client O2 logs + nœud `event-log` + page Événements | Événement de flow visible et cherchable |
| **W2** | Kind `model`, `ml_models`, crate `pnex-vision`, test sur image | YOLOX-nano détecte une personne sur image fixe |
| **W3** | Nœud `vision-detect` + snapshot | Détection live ESP32-CAM → événement → notif |
| **W4** ✅ | D86 : journal notifications → O2 + onglet | E2E réel (flow → webhooks + bus, UI) |
| **W5** | D100–D104 : modèle vérifié à l'enregistrement, flow réveille la caméra, statuts des nœuds, test live | ESP32-CAM réelle : personne détectée en live (/models) et dans un flow, statuts visibles |

## 6. Ordres de grandeur

ESP32-CAM VGA qualité 12 ≈ 15–30 Ko/frame. À 5 fps : ~100 Ko/s ≈
**360 Mo/h** par caméra enregistrée en continu. Défauts prudents :
`on_demand`, 5 fps, segments de 60 s, rétention 7 jours. Au-delà (H.264,
détection de mouvement pour n'enregistrer que l'utile) : décision
explicite ultérieure (dépendance ffmpeg, `ml-vision.md`).

## 7. Non-goals (phases V/W)

- Transcodage H.264/HLS, audio ;
- sources RTSP/ONVIF (le header `PXC1` et le CameraHub sont agnostiques :
  un ingesteur RTSP côté worker publiera sur le même bus plus tard) ;
- entraînement de détecteurs (Model Lab, axe C) ;
- inférence sur l'ESP32 lui-même.

## 8. État d'implémentation (2026-09-29)

| Brique | Commit | Validation |
|---|---|---|
| Contrat `pnex-core` (`camera`, `avi`, `CameraConfig`) | 1dfbb8f | tests unitaires (vecteur doré `PXC1` partagé avec le C++) |
| Firmware `generic_esp32cam` + module caméra lib PneX | c96e5ef | compilation 5 cibles (esp32cam 54 % flash), tests hôte 9/9 — **pas flashé** |
| Backend `/ws/camera`, CameraHub, live, réglages, segments | (V1/V3 backend) | intégration : frame chiffrée → viewer, snapshot, anti-clone 4003, PATCH poussé live, segment fs listé/servi/supprimé |
| Nœuds `camera-source` + `video-record` | (V3) | E2E runtime réel : Valkey → AVI lisible (≥ 5 frames, 10 fps restitués) |
| Événements O2 (client `_json`/`_search`, API, nœud `event-log`) | (W1) | aller-retour O2 réel (recherche plein texte + niveau, payload intact) |
| `pnex-vision` (tract 0.23, YOLOX) | b66f3cb | YOLOX-nano sur l'image de référence : dog 0,83 · car 0,81 · bicycle 0,81 |
| Registre `ml_models` + test sur image | fb2fe0b | API réelle : upload `.onnx` → modèle → détection |
| Nœud `vision-detect` | f6a730c | E2E runtime réel : frames Valkey → détections dog/bicycle |
| Nœuds `event_log` / `vision_detect` dans le graphe | 88196a9 | tests projection + validation |
| UI `/cameras` (live, réglages, enregistrements + lecteur) + nœuds éditeur | 758e7f6, d302d87 | E2E Playwright fr/en sur serveur dédié (live WS ouvert, PATCH, lecteur AVI) |
| UI `/models` (test sur image, boxes) + `/events` + nœuds éditeur | c067f0d | E2E Playwright : YOLOX-nano uploadé, dog.jpg → 4 boxes |
| W5 backend/runtime (D100–D103) : introspection/validation ONNX, colonnes de check (000041), `inspect`/`check`/`test-live`, demande caméra des flows, statuts `pnex-status`, garde de deploy | 631041a | tests : nano 416 imposé à une spec 640, `label_count:80`, modèle cassé refusé (422 `ml-model-invalid`) ; demande caméra depuis l'artefact |
| W5 UI (D100–D104) : statut/vitesse/Vérifier, formulaire verrouillé, modal test live, badges d'état des nœuds | 0aabe45 | build wasm + gardes i18n ; E2E réel en cours |

Écarts / précisions par rapport aux décisions :

- **Keepalive `/ws/camera`** : le firmware envoie `PING` en clair toutes
  les 15 s (watchdog serveur 45 s) et répond aux pings WS.
- **`video_segments.flow_id`** est un BIGINT (`flows.id`), pas un UUID.
- **Bus Valkey clé par slug** (`device_registries.device_id`) comme le
  cache last-value et les configs de nœuds.
- **Frames au format fil** : le serveur jette une frame si le déchiffrement
  donne un header invalide (mauvaise clé) — compteur, log en puissance de 2.
- **Profil board AI-Thinker** : seuls GPIO4 (LED flash) et GPIO33 (LED
  rouge, ligne virtuelle) sont admis ; **orientation USB non confirmée**
  sur carte réelle (index 0 = extrémité 5V/3V3).
- Live : le front repousse une URL WS à jour (jeton rafraîchi) toutes les
  20 s pour les reconnexions.
- Limites connues : étiquettes de boxes qui se chevauchent dans le test
  d'image ; page Événements jamais vue avec des données réelles en E2E UI ;
  live jamais vu avec une vraie caméra.
- D86 livré (journal notify → O2, onglet Événements de /notifications).
- Pas encore : snapshot annoté en média (option D83),
  transcodage/détection de mouvement (non-goals).


## 9. Reprise — E2E matériel ESP32-CAM (checklist)

État au 2026-09-29 midi : tout est sur `main` (dernier commit docs 89c8265),
rien de poussé. Le firmware compile, n'a jamais été flashé.

### 9.1 Remettre la stack à jour

1. Le conteneur `pnex-server` est **antérieur** aux migrations 000035
   (`device_cameras`, `video_segments`) et 000036 (`ml_models`) :
   `task docker:build` puis `task app:up` (migrations au démarrage), **ou**
   stack de dev `task dev:backend` (vérifier `/proc/<pid>/exe` non
   `(deleted)`, cf. mémoire « process sur binaire supprimé »).
2. Re-seed pour avoir le board `esp32cam-ai-thinker`, son profil et le
   predefined `generic_esp32cam` (+ `description_i18n`) : le seed met à jour
   les lignes existantes.
3. Le runtime de flows doit voir `VALKEY_URL` et le jeton de service
   (`PNEX_FLOW_RUNTIME_TOKEN`) : sans eux, les nœuds caméra échouent au
   deploy (erreur explicite `camera_bus_unavailable` / URL absente).

### 9.2 Créer et flasher

1. UI → Devices → nouveau device, preset **Generic ESP32-CAM**
   (`generic_esp32cam`), build serveur (bouton existant).
2. Flash : l'AI-Thinker n'a **pas d'USB** → carrier ESP32-CAM-MB (USB) ou
   FTDI 3,3 V/5 V (TX↔U0R, RX↔U0T), **GPIO0 à la masse au reset** pour
   le mode download, puis retirer GPIO0 + reset pour démarrer.
   Build manuel possible : `task fw:flash FIRMWARE=generic_esp32cam` avec
   les 6 variables base64 + `PNEX_PIO_BOARD=esp32cam
   PNEX_BOARD_NAME=esp32cam-ai-thinker PNEX_FW_VERSION=1` et les 9
   `PNEX_SCREEN_*` à 0/-1.
3. Monitor : `task fw:monitor FIRMWARE=generic_esp32cam PORT=/dev/ttyUSB0`
   (rappel : ouvrir le port série = reset ; zombie anti-clone 4003 possible
   juste après un reset série → attendre le watchdog 45 s).

### 9.3 Points à vérifier (dans l'ordre)

| # | Vérification | Où regarder | Si KO |
|---|---|---|---|
| 1 | `esp_camera_init` OK, PSRAM détectée | monitor série | alim 5 V ≥ 500 mA (brown-out fréquent sur FTDI 3,3 V) ; flags PSRAM du `platformio.ini` |
| 2 | Announce avec cap `camera` → ligne `device_cameras` | logs serveur « manifeste annoncé », `GET /api/v1/cameras` | cap annoncé seulement si l'init caméra a réussi |
| 3 | Ouvrir le live dans `/cameras` → `CameraConfig{enabled:true}` → `/ws/camera` connecté | badge streaming, logs | 4001/4006 = token/clé ; 4003 = session fantôme |
| 4 | Images visibles, fps conforme | live | frames rejetées = mauvaise clé (log en puissance de 2 « bad frame ») ; fps trop haut = garde serveur +50 % |
| 5 | Changer framesize/quality/fps → appliqué à chaud (Ack) | live + monitor | taille max UXGA : buffers initialisés à UXGA |
| 6 | Fermer le live → arrêt après 30 s (`on_demand`) | badge | — |
| 7 | Passer en `continuous`, flow `camera-source → video-record` (segment 30 s) → segments dans l'onglet Enregistrements, lisibles dans le lecteur et VLC | `/cameras` → Enregistrements | `PNEX_FLOW_VIDEO_URL` absent = token runtime manquant |
| 8 | Flow `camera-source → vision-detect (person) → event-log` → événements dans `/events` | `/events` | modèle : uploader `~/.cache/pnex-vision-test/yolox_nano.onnx` dans `/models` |
| 9 | Profil board : sens USB/connecteur, LED flash GPIO4 et LED rouge GPIO33 (active LOW) pilotables | éditeur pinout, write pin | corriger `fixtures/devices/board_profile_esp32cam.yaml` puis reseed |
| 10 | OTA (min_spiffs, slots 1,9 Mo) | bouton OTA | — |

Risques identifiés non mesurés : double copie de la frame dans
`sendBinary` (ArduinoWebsockets → `std::string`, en PSRAM), marge mémoire
en UXGA, chemin wss/CA sur la WS caméra.

### 9.4 Tests automatisés à relancer

```
cargo test -p pnex-backend --test ws_camera --test ml_models --test events
PNEX_VISION_TEST_DIR=~/.cache/pnex-vision-test cargo test -p pnex-vision -- --include-ignored
PNEX_VISION_TEST_DIR=~/.cache/pnex-vision-test cargo test -p pnex-flow-runtime --test camera_record --test vision_detect -- --ignored   # Valkey local
cargo test -p pnex-backend --test events -- --ignored   # O2 local
```
