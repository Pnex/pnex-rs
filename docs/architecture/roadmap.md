# Roadmap produit

> Consolidation du 2026-09-23. Ce document est la **vue de pilotage** : il
> agrège l'état réel des domaines (docs de domaine + journal + mémoire).
> **Les docs de domaine restent la source de vérité du détail** ; cette vue
> se met à jour à chaque jalon livré. Rédigée après un état des lieux où
> firmware ESP, flows et fonctions sont consolidés — l'effort se déplace
> vers le reste.
>
> **Revue du 2026-10-01** : resynchronisée avec l'historique (agent local,
> caméra V+W, nœuds prédictifs, firmware custom, cluster D106–D108 livrés
> depuis la création). Priorité recommandée : un **sprint de
> consolidation court** (E2E cluster, banc matériel — N5 fermé par le
> coffre de secrets le même jour) avant le
> prochain gros chantier (extension navigateur). Nouveau tronc : la
> **palette de nœuds par catégories** (P1.7, D109) puis la **parité
> n8n** (P2.11).

## Méthode

- Deux horizons imbriqués : **consolider** (fermer le livré non validé)
  avant **étendre** (nouveaux chantiers).
- Règle « livré ≠ fini » : une feature est finie quand elle est validée
  E2E réel (web headless pour le web, banc matériel pour le firmware).
- Trois critères de tri : (1) dette de validation accumulée, (2) trous
  user-facing dans des troncs déjà livrés, (3) nouveaux chantiers dont la
  spec est prête.

## Axes produit (directive 2026-09-23)

Trois axes demandés explicitement. Aucun n'a encore de PRD — la prochaine
étape de chacun est son doc de conception (même chemin que les autres
domaines : PRD → tranchages → implémentation).

### Axe C — IA : vision + prédictif (labo modèles + inférence)

Ambition : toutes les fonctions caméra et la reconnaissance par
IA/modèle, **fabrication de modèle incluse** — et le prédictif sur les
données collectées.

- **Model Lab** (petit labo) : projets par org, **datasets de deux
  familles** — images (upload, réutilise la bibliothèque média D21,
  annotation) et **séries temporelles O2** (sélection de séries
  télémétrie → dataset) ; versionnage des datasets, lancement
  d'entraînement, évaluation, modèle réutilisable (registre de modèles
  versionné, école `media_assets`).
- **Vision** — premier cas concret : détection de numéro — relever des
  chiffres de compteur, généralisable à tout OCR de nombres courts.
- **Prédictif** — les données collectées en volume (séries O2) servent à
  entraîner des modèles de **forecasting** (dérive lente, usure) et de
  **détection d'anomalie**, corrélés aux **alertes** (canaux notify
  N1–N4 déjà livrés, nœud flow notify) : l'idée en réserve « anomalie
  persistante » d'notifications.md devient un cas d'usage du scoring
  modèle → notification.
- **Inférence** : charger un ou plusieurs modèles du registre — sur un
  **flux vidéo** (boxes, classes, valeurs) ou sur le **flux de
  télémétrie** (score, prévision) — sorties → flows (séries O2,
  notifications, règles). Cible à trancher : serveur (runtime
  d'inférence backend/worker), navigateur (wasm/webgpu), edge (agent
  local C2 sur mini-PC, ou ESP32-S3 pour modèles minuscules).
- **Stack proposée (pilier `ml-vision.md`, statut « proposé »)** :
  tout-Rust, sans Python côté plateforme — **Burn**/`burn-import` +
  **tract** (inférence ONNX, à benchmarker l'une contre l'autre sur Pi),
  **linfa** (ML classique), **augurs** (séries temporelles : forecast,
  anomalie), `ffmpeg-next`/`gstreamer-rs` (seule dépendance native
  assumée, à valider sur ARM). Les capacités ML sont des **nœuds natifs
  du moteur de flows** (schéma dans `pnex-core`) : inférence vision,
  anomalie, forecast, modèle linfa. Modèles pré-entraînés d'abord
  (YOLOX/RT-DETR Apache-2.0, jamais YOLOv8/v11 AGPL) ; l'entraînement de
  détecteurs est hors scope au départ — le labo démarre par le ML
  classique linfa.
- Tranchages restants : cible d'inférence (serveur / edge / selon
  charge), stockage des modèles (RustFS vs Postgres, école
  `media_assets`), sources vidéo V1 (webcam via extension/navigateur,
  RTSP, ESP32-CAM), burn vs tract (POC étape 1 du pilier).

### Axe G — Acquisition position GPS (sans app iOS/Android dédiée)

Contrainte actée : **pas de nouvelle application dédiée** — l'acquisition
passe par les surfaces existantes.

- **G1 — firmware générique** : nouvelle famille de capacités `gps`
  (module NMEA sur UART, école NEO-6M/M8N), télémétrie
  lat/lon/alt/vitesse/HDOP → séries O2 ; positionnement du POI device sur
  /map (interaction avec les placements D43).
- **G2 — web/APK existant** : API Geolocation du navigateur — « me
  localiser » sur /map, création de POI à la position courante, géotag
  des captures Take 360 (métadonnées EXIF/GPano).
- Ouverture (après G1/G2) : geofencing — un nœud flow lit la position,
  calc entre dans zone/hors zone → notify.

### Axe X — Extension navigateur (C1b → C3) : élevé

La spec est écrite (`extension-collector.md`, 10 décisions tranchées) ;
la directive précise la sémantique attendue : **collecte en temps réel et
refresh auto d'un site** — content-script avec polling configurable
(refresh auto) et détection d'événements quand le site s'y prête
(MutationObserver/réseau), transmission en WS device → pnex. Valider la
spec avec ces deux points, puis implémenter (MV3, consentement par
tâche).

## État par domaine (2026-09-23, revu 2026-10-01)

| Domaine | État | Reste principal |
|---|---|---|
| **Scale horizontal** (D106–D108) | **Livré 2026-10-01** — flows shardés par org (contrôleur élu, fencing), bus device Valkey par pod, présence device en baux Valkey (Valkey obligatoire), audit multi-pods scale-a→e (quotas atomiques, versions optimistes, listes paginées SQL, single-flight) ; E2E 2 processus | **E2E multi-réplicas réel** (k8s : bascule contrôleur, perte de pod, rolling update) ; manifests k8s dans `pnex-deploy` ; `LAST_VALUES` encore local au pod (flow-engine §7) |
| **Palette de nœuds** | **Sections par catégorie livrées** (D109, P1.7) : 9 catégories en ordre fixe, recherche transverse | **P2.11** catalogue piloté par descripteurs (parité n8n) |
| **Firmware ESP générique** (F1/F2) | Consolidé — transport commun dans la lib `firmware/lib/pnex`, `Announce.caps`, StateReport par capacité ; flash F1/F2 validé réel (NodeMCU, ESP32-C3 0639ee4) | **F3 firmware `regulator`** (persist D45, buffer D46, boucle) — hardware requis ; e2e carte réelle Brick 0 jamais flashée ; OTA sur ESP32 réel (8266 OK) |
| **Flow engine** | Consolidé — ETL full-Rust, device read/write (routage topic 90f23e0), calc, debug, http_fetch (C1a), nœud notify, cache live Valkey (b966c1a→e7257f8), **exclusivité write par pin** (8d60950), nœuds caméra/vision/mémoire/prédictifs, cluster D106 ; deploy de la version affichée + présence multi-utilisateurs dans l'éditeur (2578ffe) | Palette par capacité (ouverture §12, après P1.7 livré), E2E matériel du write/PWM, ack des commandes non persisté (brick0 §10) |
| **Fonctions custom** | Livré — JS (rquickjs) + Starlark (`_` privé, json en extension) | Limite acceptée : interrupt handler rquickjs ; payload remplacé (pas merge) — rien d'ouvert |
| **Notifications** | N1–N4 livrées (7 canaux, templates, anti-spam, canaux dédiés D61–D65), D66–D68 (trigger booléen), journal O2 D86, fan-out `/ws/notify` multi-pods | ~~N5 chiffrement AEAD des secrets~~ fermé par le coffre de secrets (2026-10-01) ; N6 points d'extension, canal gotify en réserve |
| **OTA** | Livré + E2E réel 8266 (2 cycles ~23 s, boucle 4003 anti-clone) | ESP32 réel ; politique de re-flash quand v2 du `.bin` (brick0 §10) |
| **Devices / pins** | Consolidé — CRUD, catalogue, quotas, pinout SVG v2, board variants, placements D43 | E2E matériel (write/PWM, fraîcheur read via Valkey) |
| **TFT / écrans** | V2 livré — barre dès boot, timeline OTA, panneau 21 pins (8e69519) | E2E matériel (tft_dev) |
| **Média (D21) + Take 360** | Couche 1 livrée ; Take 360 V2 serveur + V3 guidage device-validé + auto-cal focale | Capture portrait 2 anneaux, résolution 1080+, iOS/web, calibration par device |
| **Viz** (carte POI-first D35–D39, dashboards D40/D41) | Mergés ; tables dormantes supprimées (migration de base D120, 2026-10-01) | Décisions §9 : style de tuiles configurable, widget psychrométrique/Mollier ; E2E interactif + Android |
| **Studio** (tours + annotations) | Mergés (0aae2ad, 9e78241) | Maquette 3D, LRU panoramas, éditeur vue initiale, export offline ; annotations : reste couche ETL/device |
| **Assistant IA v1** | Implémenté — 10 outils fermés, devices read-only (A4) | Tranches suivantes non spécifiées |
| **Collection C1a** | http_fetch livré (2026-09-15) | — |
| **Extension navigateur (C1b)** | **Spec écrite** (`extension-collector.md`, 10 décisions) — implémentation = jalon C3 | Implémentation après validation de la spec |
| **Agent local (C2)** | **Livré** (2026-09-30) (`edge-agent.md`, D95–D99) : API locale libre, file disque, enrôlement une commande, Linux musl + Windows | Test Windows réel, macOS, pont WS devices LAN |
| **M2M résilient (M1–M3)** | PRD posé (`m2m-resilient.md`) | M1 POC zenoh-pico WiFi (zéro achat), M2 6LoWPAN à prototyper avant engagement |
| **Android** | Build + login PKCE E2E validé ; storage persistant ; scan LAN ; APK arm64 publié à chaque `main` vert (CI Apps, pré-release `nightly`, signé debug → sideload) ; bouton Flash masqué (`flash::offered()`), package `io.pnex.app` (`[bundle]` Dioxus.toml) | Keystore de release (Play Store) ; libellé de l'app encore dérivé du nom du crate |
| **Desktop natif** | **Livré 2026-10-01** — Linux x86_64 / aarch64 et Windows x86_64 (cross mingw) : login, flash USB par esptool embarqué ; validé mainteneur sur Linux et Windows ; publié à chaque `main` vert (CI Apps, pré-release `nightly`), smoke test de démarrage sur un runner Windows | Paquets installables (.deb/.msi), signature de code, macOS |
| **Schéma de base** (D120) | **Livré 2026-10-01** — une migration de base (SQL PG + SQLite), reliquats pré-coffre retirés, dérive SQLite corrigée (tables mortes, `map_pins`, FK manquantes), test de parité PG/SQLite bloquant (`migrations.md`) | Palier SQLite : E2E réel de bout en bout (jamais déroulé sur une install SQLite) |
| **Organisations (D42)** | Backend livré (labels, containment, edges) | **UI arbre** (couches org dans l'UI) |
| **Auth (D19)** | Rauthy livré, branding API, password grant, OIDC natif | — |
| **CoolProp** | In-process mergé (d126225) | — |
| **Brick 0** (prototypage rapide) | PRD (`brick0.md`) | e2e carte réelle, quota mixed 0→1 à valider, persistance ack |
| **Caméra & vidéo** (D73–D86, D100–D104) | **Phases V et W implémentées** (2026-09-29/30) : `/ws/camera`, CameraHub, `camera-source`/`video-record`/`vision-detect`/`event-log`, registre `ml_models` (tract YOLOX), fiabilité W5 ; flash web ESP32-CAM-MB validé, stream 5 fps stable (XCLK 16 MHz) | E2E complet caméra → vision → événement → notification sur carte réelle |
| **Workers / fabric** | 2 workers Loco CPU co-localisés sur le control plane (`build_firmware`, `stitch_panorama`) ; claim idempotent + timeout par job (stitch, 2026-10-01) ; **PRD fabric proposé** (`worker-fabric.md`, 2026-09-25) | Validation du PRD (P1.6) puis MVP (P2.7) — pré-requis de tout job GPU (Gaussian Splatting, axe C) |
| **Prédictif** (anomaly/forecast) | **Livré** 2026-09-29 (`pnex-node-predict` : BOCPD maison + forecast) — étape 3 du pilier ML/Vision | Branchement documenté anomalie → notify (cas « anomalie persistante ») ; E2E sur séries réelles |
| **Firmware custom** (D87–D94) | **Livré, désactivé par défaut** — L1, L3, L4, L5 : API `addMetric`/`publish`/`onCommand`, projets + révisions, catalogue de libs, Verify en job de file, firmware choisi au provisioning (1→N devices, build de la dernière révision) ; L2 partiel (bwrap optionnel, pas actif en stack dev) | Sandbox D91 réelle, magasin de libs offline, nœud flow des commandes custom, révision déployée sur la page device (D94), E2E BME280 → O2 → flow → notification |
| **Coffre de secrets** (D110–D119) | **Livré 2026-10-01** — S1–S8 (`secrets.md`) : XChaCha20-Poly1305, trousseau en env, références typées, runtime qui résout par id, rôle `member`, rotation + rechiffrement | Clés device et tokens agent hors coffre (chiffrement au repos possible plus tard) |
| **API publique** | **Rien** — aucune API destinée aux programmes tiers en dehors de l'agent edge (push de valeurs) | Horizon P3 (abonnements live, jetons à portée, REST documentée) |
| **Mémoire d'org** (Valkey) | Livré — `memory-write`/`memory-read` par org, source « Mémoire » des widgets | — |

## P0 — Consolidation : fermer le livré non validé

Une grosse part du travail récent est livré mais attend le banc matériel —
la dette de validation s'accumule sur **une seule cause**, donc une passe
consolidée la dissipe d'un coup.

### P0.1 — Passe E2E matériel consolidée

Un banc (ESP32, NodeMCU, TFT, relais) déroule la liste :

1. **Write pin + PWM** (duty 0-100) sur device réel — le write routé par
   topic (90f23e0) et l'exclusivité write (8d60950) n'ont jamais vu de
   carte.
2. **OTA sur ESP32 réel** (le 8266 est validé, l'ESP32 pas).
3. **TFT** : barre dès boot, timeline OTA, panneau de pins (tft_dev).
4. **Read** : fraîcheur cache-first Valkey contre un device réel.
5. **Écran debug local** (ScreenChoice) et Brick 0 sur carte réelle (DoD
   §9 de brick0.md — à vivre avec l'utilisateur).
6. **Chaîne caméra complète** (ajout 2026-10-01) : ESP32-CAM →
   `camera-source` → `vision-detect` → `event-log` → notification.
7. **Firmware custom** (ajout 2026-10-01) : `main.cpp` utilisateur →
   build isolé → flash/OTA → métrique dans O2 (E2E cible de
   `custom-firmware.md`).
8. **F3 regulator** (P1.3) sur le même banc.

Critère de sortie : chaque entrée « reste E2E matériel » de la mémoire et
des docs est levée ou convertie en ticket précis. Mise à jour des docs de
domaine + roadmap.

### P0.2 — N5 : chiffrement AEAD des secrets de notifications

> **✅ Livré le 2026-10-01** par le coffre de secrets (S1–S8,
> `secrets.md`). Section conservée pour l'historique.

> **Élargi le 2026-10-01** en coffre de secrets par org
> (`secrets.md`, D110–D118, tranché) : notifications, nœud HTTP, WiFi,
> fournisseurs LLM en CRUD, rôle `member`. N5 est fermé par le lot S4 ;
> ordre des lots S1 → S2 → S4 → S5 → S6 → S7 → S3 → S8.

Aujourd'hui les secrets de canaux (tokens ntfy/telegram/slack/smtp) sont
en clair en base. Chantier **borné** : XChaCha20-Poly1305 avec clé env
partagée backend↔engine, masquage write-only (école `deserialize_some`),
**rotation de clé documentée** (critère de sortie déjà écrit dans
notifications.md §11). La règle i18n s'applique : erreurs machine + clés
fluent des deux locales.

### P0.3 — E2E du cluster sur multi-réplicas réels (ajout 2026-10-01)

Le chantier scale (D106–D108 + audit scale-a→e) est validé en tests et
en E2E 2 processus, jamais sur un vrai déploiement multi-réplicas. Règle
« livré ≠ fini » : dérouler sur k8s (≥ 2 pods API+worker, Valkey) la
bascule du contrôleur élu, la perte brutale d'un pod (fencing, reprise
des orgs), un rolling update (drain ≥ 40 s), et un device connecté au
pod A dont le flow tourne sur le pod B (bus D107). Livrable annexe : les
manifests k8s dans `pnex-deploy` (flow-engine §7, point 6).

## P1 — Compléter les troncs inachevés

### P1.1 — Organisations : UI arbre (D42)

Le backend (labels GIN, containment, edges) est livré ;
il manque l'UI (arbre des couches org). Sans elle, la couche org n'existe
pas pour l'utilisateur — c'est le plus grand trou user-facing du socle.

### P1.2 — Android « distribuable »

> **Étapes 1–2 livrées le 2026-10-02** (branche
> `feat/roadmap-android-distrib`) — APK à valider sur téléphone.

1. ~~Masquer le bouton « Flasher »~~ — `flash::offered()` (faux sur
   Android uniquement) masque le bouton de la liste des devices ;
   l'assistant affiche à la place « flashez depuis un ordinateur ».
   Firefox/Safari et le desktop sans esptool gardent le bouton et
   l'explication du modal ;
2. ~~Branding APK~~ — `[bundle] identifier = "io.pnex.app"` dans
   Dioxus.toml (sortie du placeholder `com.example.PnexFrontend`). Une
   APK installée avant ce changement est une autre app : la désinstaller.
3. Reste : keystore de release (Play Store), libellé de l'app.

### P1.3 — F3 : firmware `regulator` (D20)

Prouve l'autonomie du bord : persistence Config (D45), buffering D46,
boucle de régulation, golden vectors déjà en CI (`pnex-core-cpp` ne peut
pas dévier sans casser la CI). Hardware requis — se combine naturellement
avec la passe P0.1 (même banc).

### P1.4 — Annotations : couche ETL/device

Le backend v4 (ensembles-tour, studio) est livré ; reste la partie
ETL/device (`device_pk null` aujourd'hui).

### P1.5 — PRD des axes produit C / G / X

Rédiger les trois docs de conception (l'axe X = relecture de la spec
existante avec les sémantiques refresh auto + temps réel). Livrables :
décisions tranchées (stack d'entraînement, cible d'inférence, sources
vidéo V1 ; module GPS choisi ; plan de tranches labo), prêtes à
implémenter en P2. Ce travail débloque le cœur de la directive produit —
il précède toute ligne de code des axes.

### P1.6 — Fabric de workers : valider le PRD (`worker-fabric.md`)

Infra transverse, **pré-requis de tout worker lourd** (Gaussian
Splatting, jobs GPU de l'axe C). Principe : **un seul runtime worker**,
routage des jobs par **capabilities** (le worker annonce, le job exige,
le scheduler matche — même modèle que les devices), enregistrement
**join-by-token** façon runner GitLab CI, polling sortant (NAT-friendly),
comm qui suit la frontière de confiance (mesh WireGuard entre ressources
du même propriétaire ; API de job authentifiée entre frontières
différentes, jamais de Postgres exposé). Les workers `build_firmware` et
`stitch_panorama` deviennent des profils de capabilities (cas dégénéré
in-process), le gpu-worker un profil de plus — pas de chemin de code
parallèle. Livrable : tranchages de la section 13 du PRD (tuning du
lease, adressage du compute en un seul champ de config + repli
gracieux, secrets du join token) et **vérification que la queue Loco
couvre claim atomique + lease/heartbeat/reaper** (sinon : le coder).

### P1.7 — Palette de nœuds par catégories (D109, ajout 2026-10-01)

> **✅ Livré** (`PALETTE_CATEGORIES` dans `flow_editor/canvas/palette.rs`,
> constaté le 2026-10-02). Section conservée pour l'historique.

Le popover `+` de l'éditeur de flows liste 26 nœuds à plat : on ne
distingue plus ce qui relève des devices, de la régulation, du code, de
l'IA… **D109** : chaque kind appartient à **une catégorie** déclarée au
même endroit que son icône et ses libellés (`flow_editor/canvas/palette.rs`),
la palette affiche des **sections titrées dans un ordre fixe**, la
recherche reste transverse (les sections vides disparaissent). La
catégorisation est de la data plate — c'est la première brique du
catalogue de P2.11.

| Catégorie | Nœuds |
|---|---|
| Déclencheurs | inject, camera-source |
| Devices | device-read, device-write, display |
| Régulation | reg-tt-heat, reg-tt-cool, reg-pid |
| Données & calcul | value, calc, json-split, json-merge, coolprop |
| Code | function, red |
| Stockage & séries | metric, memory-write, memory-read, video-record, event-log |
| IA & prédictif | vision-detect, anomaly, forecast |
| Intégrations | notify, http-fetch |
| Debug | debug |

Ordre des catégories = parcours d'un flow (source → traitement → sortie).
La couleur des nœuds au canevas reste par kind dans cette tranche.

## P2 — Nouveaux chantiers prêts (spec écrite)

### P2.1 — Extension navigateur (C1b → jalon C3) — **élevé (directive produit)**

Implémentation de `extension-collector.md` amendée par la directive :
WS device unique, crypto WASM pnex-core, entité `extension_rules`,
`ServerMsg::RulesConfig`, OIDC plan gestion / device token plan données,
connect-on-wake, cast ≠ apply — plus **refresh auto configurable** et
**collecte temps réel** (content-script, MutationObserver quand possible).
MV3 + consentement par tâche. Pré-requis : validation de la spec amendée
(P1.5).

### P2.2 — Axe G : GPS (G1 + G2), après PRD

G1 : capacité `gps` firmware (parse NMEA côté transport/cap, télémétrie
→ O2, POI device suivie sur /map). G2 : Geolocation côté web/APK
localisation + création POI + géotag Take 360. Aucune app dédiée.

### P2.3 — Axe C : pilier ML/Vision (après validation du pilier)

Suivre les 4 étapes de `ml-vision.md` :

1. **POC inférence** — YOLOX ONNX via Burn **et** tract sur image fixe,
   bench sur Pi → tranche burn vs tract. **De fait : tract** en
   production (`pnex-vision`, phase W de P2.8) ; le bench burn reste à
   faire seulement si tract plafonne.
2. **Pipeline vidéo** — RTSP → échantillonnage frames → nœud inférence →
   détections dans O2.
3. **Télémétrie** — nœuds **anomalie** et **forecast** sur les
   séries existantes → notifications (canaux N1–N4) : le prédictif
   branché aux alertes. **Livré 2026-09-29** (`pnex-node-predict`,
   changepoint BOCPD maison : celui d'augurs dépend de bincode).
4. **ML classique** — nœud linfa + stockage/versionnage des modèles
   (RustFS ou Postgres, école `media_assets`).

Le **Model Lab** (datasets images + séries O2, annotation, entraînement
linfa, registre) s'appuie sur l'étape 4 et reste la tranche suivante ;
le fine-tuning de détecteurs est une décision explicite ultérieure.

### P2.8 — Caméra & vidéo (`camera-video.md`, D73–D86) — **phases V+W implémentées** (2026-09-30)

Déclencheur : ESP32-CAM commandé (2026-09-29) — tranche « sources vidéo
V1 » de l'axe C. **Phase V (collecte)** : canal binaire `/ws/camera`
chiffré par la clé device, CameraHub (live navigateur event-driven, sans
cron ni stockage), bus Valkey vers le moteur de flows, nœuds
`camera-source` (premier nœud source événementiel) et `video-record`
(segments MJPEG-AVI découpés par durée/taille/gap, rétention, stockage
fs ou S3/RustFS via le `MediaStore` du backend), table `video_segments`,
firmware `generic_esp32cam` (AI-Thinker), page `/cameras`. **Phase W
(vision + événements)** : registre de modèles (média `kind=model` +
`ml_models`, CRUD + test sur image), crate `pnex-vision` (tract,
YOLOX Apache-2.0), nœud `vision-detect`, **événements JSON en logs O2**
(nœud `event-log` + page Événements — jamais en Postgres/sqlite).

### P2.9 — Journal des notifications → OpenObserve (D86) — **livré 2026-09-29**

Sortir `notify_deliveries` de Postgres/sqlite : stream O2 logs
`notify_deliveries` alimenté par **tous** les envois (y compris webhooks
du nœud flow, aujourd'hui non journalisés) avec le statut retour ; la
rétention devient celle d'O2 (plus de pruner). UI : onglet
**Événements** dans /notifications (canaux · templates · événements) —
liste complète des notifs avec statut, erreur, source, flow. Réutilise le
client O2 logs de la phase W1 (P2.8). Livré : table supprimée
(migration 000037, sans reprise des lignes), writer de fond + route
interne `/internal/notify/journal` pour le nœud, badge par requête O2
agrégée, onglet Événements ; détail `notifications.md` §15.

### P2.10 — Firmware custom : IDE intégré (`custom-firmware.md`, D87–D94) — **en cours** (L1/L3/L5 livrés, L2/L4 partiels)

Écrire son propre `main.cpp` dans l'UI (lib PneX complète, board du
device, libs d'un catalogue épinglé), versionné en base (révisions
immuables, build → révision), compilé par le worker existant, déployé en
flash USB ou OTA. Prérequis n°1 : API lib `addMetric` / `publish` /
`onCommand` (D87 capacité `metric` + `StateReport.cap_id`, D88
`ServerMsg::Command`). Non négociable dès la V1 : worker de build
**isolé** (D91 — pas de secret serveur, réseau coupé, FS en lecture
seule ; compiler du C++ utilisateur = exécuter son code). Offline =
catalogue préchargé dans l'image worker (D92) ; libs uploadées / registre
ouvert / mode strict : plus tard. Lots L1 (lib) et L2 (sandbox) utiles
seuls ; E2E cible : BME280 I2C → O2 → flow → notification.

### P2.4 — Studio : tranches suivantes

Dans l'ordre écrit (studio.md §5) : maquette 3D (`tours.mode = maquette`),
préchargement/LRU des panoramas, éditeur de vue initiale « depuis le
panorama », export offline non planifié.

### P2.5 — Take 360 : suite

Portrait 2 anneaux (16 à plat + 12 incliné), montée en résolution
(720 → 1080+ sans downscale), calibration par device.

### P2.6 — Viz : trancher les décisions §9

1. ~~Sort des tables dormantes~~ : **supprimées** le 2026-10-01 par la
   migration de base (D120, `migrations.md`).
2. Style de tuiles configurable (constante/env front, repli vieux
   webviews sans WebGL1 à mesurer).
3. Widget psychrométrique/Mollier (+ CoolProp in-process déjà mergé —
   source de propriétés à trancher).

### P2.7 — Fabric de workers : MVP (après P1.6)

Phase MVP de `worker-fabric.md` §12 : runtime worker unifié + routage par
capabilities sur la queue PG existante ; **un seul** archetype distant
(control léger + worker GPU distant via mesh WireGuard) ; join token
single-use + enregistrement par capabilities ; lease/heartbeat/reaper ;
360 et firmware repliés dans la fabric **sans changement fonctionnel**.
Plancher GPU = **Vulkan** (Brush/wgpu), CUDA jamais un gate ; split des
plans ⇒ un endpoint S3 (RustFS par défaut, jamais MinIO, jamais une
dépendance dure) ; all-in-one = FS + DB locaux. La progression live
(SSIM/steps) passe par O2, pas par la queue. Débloque la feature Gaussian
Splatting et les jobs GPU du pilier ML/Vision (P2.3). v2 (générateur de
script d'install Debian → K8s → Windows, UI de fleet Dioxus, pools
GitOps + HPA) et v3 (API job authentifiée, multi-tenant, compute managé
→ PRD monétisation) restent en P3.

### P2.11 — Parité n8n : catalogue de nœuds piloté par descripteurs (ajout 2026-10-01)

Ambition : reproduire à terme les fonctionnalités de n8n (déclencheurs
variés, logique de flux, transformation de données, centaines
d'intégrations, credentials). À cette échelle, la recette actuelle
« 8 points de câblage par nœud » (enum `FlowNodeKind`, palette, icône,
libellés, inspecteur, crate runtime, enregistrement, i18n) ne tient pas.
Pré-requis, dans l'ordre :

1. **Descripteur de nœud unique** (`pnex-core`) : clé, catégorie,
   ports, schéma des paramètres, icône — la palette, l'inspecteur
   générique et la validation du deploy en dérivent. Les nœuds existants
   gardent leur inspecteur dédié ; les nouveaux peuvent n'avoir que le
   formulaire généré.
2. **Palette à l'échelle** : navigation par catégorie façon n8n (liste
   des catégories → nœuds, recherche transverse), sous-catégories
   (ex. Intégrations › Communication / Données / Dev).
3. **Logique de flux** manquante : if/switch, filter, loop/batch,
   wait/delay, merge par mode (append, par clé, attendre tout),
   gestion d'erreur par nœud (retry, branche erreur, error trigger).
4. **Déclencheurs** : webhook entrant, cron (aujourd'hui dans inject),
   événement plateforme (device en ligne/hors ligne, alerte, OTA).
5. **Credentials** : coffre par org réutilisable par les nœuds
   (s'appuie sur le coffre de secrets livré, `secrets.md` — le nœud
   http-fetch l'utilise déjà).
6. **Intégrations** : HTTP générique d'abord (http-fetch enrichi :
   méthodes, auth, pagination), puis nœuds dédiés par service.

Livrable avant tout code : un PRD (`flow-nodes-catalog.md`) qui tranche
la forme du descripteur, le formulaire généré vs inspecteur dédié, et le
périmètre V1 (décision #10). Ne jamais copier code, icônes ou
descriptions de n8n (licence Sustainable Use, cf. règle des assets
tiers) : on reproduit des fonctionnalités, pas l'implémentation.

## P3 — Horizons (décisions de phase explicites)

Rien n'y est engagé ; chaque entrée exige une décision explicite (principe
« pas de glissement silencieux »).

- ~~**C2 — agent local**~~ : livré 2026-09-30 (`edge-agent.md`, D95–D99).
- **M1 — POC M2M zenoh-pico WiFi** (sans broker, parc ESP existant, zéro
  achat) : coupure hub → la boucle continue. M2 (6LoWPAN) nécessite un
  prototype MTU avant tout engagement. UI de gestion seulement à M3.
- **iOS** : décision de phase explicite (features.md). Le desktop est
  livré (2026-10-01, Linux + Windows).
- **Fabric de workers v2/v3** (`worker-fabric.md` §12) : générateur de
  script d'install + UI de fleet + pools GitOps/HPA (v2) ; frontière
  managée par API job authentifiée, multi-tenant, compute managé (v3,
  lié au PRD monétisation). La fabric enregistre et route, elle ne
  provisionne jamais (pas de fleet manager).
- **API publique pour programmes tiers** (ajout 2026-10-01) : n'importe
  quel programme **s'abonne en live** aux valeurs d'un device ou aux
  événements publiés par un flow, lit l'historique et écrit sur un pin
  (pin libre uniquement — jamais un pin piloté par un flow, règle 8d60950).
  Volet complet : **jetons à portée limitée** (lecture / écriture, par
  device ou par flow, révocables, distincts des tokens device et du JWT
  utilisateur), transport à trancher (WebSocket, SSE ou appels REST),
  API REST documentée sur les mêmes jetons. Aujourd'hui seul le push de
  valeurs existe, via l'agent edge (D95–D99). Hors 0.1.0 : la priorité
  est le contenu pour faire adhérer une communauté. Décision #11.
- **Ouvertures** : palette flow par capacité (au moment où l'éditeur
  touche aux formulaires D20), compression du fil MCU (jamais un
  prérequis), sous-titres/tours offline.

## Décisions à trancher (appartiennent au produit)

| # | Décision | Quand |
|---|---|---|
| 1 | Sort des tables dormantes viz (drop vs keep) | Après `count(*)` — avant P2.4 |
| 2 | Engagement M2M M1 (POC zenoh-pico) ou report | Après P0/P1 |
| 3 | Quota Free mixed 0→1 (brick0 §10) | À la prochaine revue de brick0 |
| 4 | Priorité relative UI arbre org (P1.1) vs banc matériel (P0.1) | Peut réordonner P0/P1 |
| 5 | Axe C : ~~stack tout-Rust vs Python~~ **tranché-proposé : tout-Rust (`ml-vision.md`)** ; ~~stockage des modèles~~ **média `kind=model` (D81)** ; ~~sources vidéo V1~~ **ESP32-CAM (D73–D77)** ; cible d'inférence V1 = serveur (D82) ; restent burn vs tract (POC étape 1) | Au POC étape 1 du pilier |
| 6 | Axe G : module GPS de référence (NEO-6M/M8N…) et surface V1 prioritaire (firmware vs web) | Au PRD axe G (P1.5) |
| 7 | Besoin réel de fine-tuning détecteurs sur données client (sinon : modèles pré-entraînés + labo linfa seulement) | Après POC (P2.3) |
| 8 | Fabric de workers : la queue Loco couvre-t-elle lease/heartbeat/reaper (sinon le coder) ; durée du lease sur jobs longs ; archetype distant du MVP | À la validation du PRD (P1.6) |
| 9 | Firmware custom : nœud flow des commandes (D88), 1er lot du catalogue (D92), technique de sandbox V1 (D91, à aligner avec P1.6) | Au lancement de P2.10 |
| 10 | Parité n8n : périmètre V1 (logique de flux, déclencheurs, credentials, quelles intégrations d'abord) et descripteur généré vs inspecteurs dédiés | Au PRD de P2.11 |
| 11 | API publique : transport des abonnements live (WebSocket vs SSE vs REST), modèle de jetons (portée device/flow, lecture/écriture, rotation), lien avec les déclencheurs webhook de P2.11 | Au passage de P3 à P2 |
| 12 | Base de dev antérieure à D120 : `db:reset` de `pnex` ou adoption de `pnex_e2e` (observations O18) | Avant la prochaine session dev |
| 13 | Jetons device dans l'URL des WebSockets (logs nginx) : masquage des logs vs auth par en-tête / premier message (O19) | Avant tout déploiement exposé |
| 14 | Version firmware par rebuild (id + compteur ou hash) pour distinguer les builds en OTA (O22) | Au prochain chantier OTA |

## Journal de la roadmap

- **2026-09-23** — Création. État des lieux : firmware ESP (F1/F2), flow
  engine et fonctions consolidés ; priorités P0 (consolidation : banc
  matériel + N5 AEAD), P1 (UI arbre org, Android distribuable, F3
  regulator, annotations ETL/device), P2 (extension C3, studio, take360,
  décisions viz), P3 horizons (C2, M1–M3, iOS/desktop).
- **2026-09-23 (directive produit)** — Ajout des axes C (caméra & vision
  IA : Model Lab, fabrication de modèles, détection de numéros, inférence
  multi-modèles sur flux vidéo), G (position GPS sans app iOS/Android
  dédiée) et X (extension navigateur : collecte temps réel + refresh
  auto → pnex, élevée en tête de P2). Nouveau P1.5 : PRD des trois axes
  avant implémentation.
- **2026-09-23 (complément)** — L'axe C couvre aussi le **prédictif** :
  le Model Lab accepte des datasets issus des **séries O2** (forecasting,
  détection d'anomalie), scoring branché aux alertes (canaux N1–N4 déjà
  livrés — l'idée en réserve « anomalie persistante » devient un cas
  d'usage du scoring modèle).
- **2026-09-23 (pilier ML/Vision)** — Stack tranchée-proposée pour
  l'axe C : **tout-Rust, sans Python côté plateforme** — Burn/burn-import
  + tract (ONNX), linfa (ML classique), augurs (forecast/anomalie),
  ffmpeg/gstreamer (décodage, seule dépendance native). Capacités
  exposées en **nœuds natifs du moteur de flows** (schéma pnex-core).
  Détail : `ml-vision.md` (4 étapes : POC YOLOX ONNX → pipeline RTSP →
  nœuds augurs → linfa + registre). MinIO rappelé banni : stockage
  modèles = RustFS ou Postgres.
- **2026-09-25 (fabric de workers)** — Ajout du PRD `worker-fabric.md`
  (proposé) : un seul runtime worker, routage par capabilities,
  enregistrement join-by-token façon runner GitLab CI, polling sortant,
  comm selon la frontière de confiance (mesh WireGuard vs API job
  authentifiée), 5 archetypes de topologie (all-in-one → compute
  managé), plancher GPU Vulkan. Pré-requis du Gaussian Splatting et des
  jobs GPU de l'axe C. Nouveaux P1.6 (validation du PRD), P2.7 (MVP) ;
  v2/v3 en P3 ; décision #8.
- **2026-09-29 (caméra & vidéo)** — ESP32-CAM commandé : PRD
  `camera-video.md` (D73–D86). Phase V : canal vidéo binaire dédié,
  live sans cron, enregistrement choisi **dans le flow** (segments
  MJPEG-AVI paramétrables → fs ou S3/RustFS), firmware
  `generic_esp32cam`. Phase W : registre de modèles + test sur image,
  inférence tract, événements JSON en **logs O2** (jamais en base).
  Nouveau P2.8 ; nouveau P2.9 : journal des notifications basculé en
  O2 + onglet Événements dans /notifications.
- **2026-09-29 (D86 livré)** — P2.9 : journal des notifications dans le
  stream O2 `notify_deliveries` (tous les envois, statut + HTTP),
  table relationnelle supprimée, O2 posé comme obligatoire dans la stack ;
  au passage, total des listes O2 (événements, journal) corrigé par un
  `count(*)` séparé.
- **2026-09-29 (firmware custom)** — PRD `custom-firmware.md`
  (D87–D94) : IDE firmware dans le navigateur, un `main.cpp` versionné en
  base, lib PneX fournie par le serveur (plus besoin du registre PIO),
  catalogue de libs épinglé et préchargé (offline), worker de build isolé
  dès la V1. Nouveau P2.10 ; décision #9.
- **2026-10-01 (revue)** — Resynchronisation avec l'historique : agent
  local C2 (D95–D99), caméra phases V+W (D73–D86, D100–D104), nœuds
  prédictifs (étape 3 du pilier ML/Vision), mémoire d'org Valkey,
  firmware custom en cours (L1/L3/L5) et chantier scale (cluster de
  flows D106, bus device D107, présence Valkey D108, audit scale-a→e)
  livrés. Constats : N5 toujours ouvert (secrets en clair), dette E2E
  matériel élargie (caméra, firmware custom), cluster jamais joué sur
  multi-réplicas réels. Ajouts : P0.3 (E2E cluster), P1.7 (palette par
  catégories, D109), P2.11 (parité n8n, décision #10). Recommandation :
  sprint de consolidation court (P0.2, P0.3, P0.1) puis extension
  navigateur (P2.1).
- **2026-10-01 (coffre de secrets)** — N5 élargi en coffre par org
  (`secrets.md`, D110–D118) : une valeur chiffrée par ligne
  (XChaCha20-Poly1305, trousseau en env), références typées sans
  templating, page CRUD centrale + édition au lieu fonctionnel, runtime
  qui résout par id (jamais de valeur dans `flows.json`), rôle `member`,
  fournisseurs LLM en CRUD (retrait sec de `PNEX_AI_*`), pas de rebuild
  en cascade au changement d'un WiFi. Au passage : `PNEX_PROD_HOST` fige
  le serveur des firmwares et des apps natives en production.
- **2026-10-01 (resynchronisation site)** — N5/P0.2 marqués livrés (coffre
  de secrets) ; firmware custom passé à « livré, désactivé par défaut »
  (L4 complet depuis le build au provisioning, reste la sandbox réelle et
  l'offline). Ajout de l'**API publique** en P3 (abonnements live,
  jetons à portée, REST documentée ; décision #11) : rien n'existe hors
  du push via l'agent edge, et la 0.1.0 reste centrée sur le contenu
  communautaire. Roadmap publique du site (`pnex-website`) alignée.
- **2026-10-01 (première release)** — Historique git ramené à un commit ;
  50 migrations fondues en une migration de base (D120, `migrations.md`) :
  tables dormantes et reliquats pré-coffre supprimés, dérive SQLite
  corrigée, parité PG/SQLite bloquante en CI. Pas de LLM plateforme
  (D119). CI sur runners GitHub (dépôt public) ; nouvelle CI **Apps** :
  desktop Linux x86_64/aarch64 + Windows x86_64 (cross) + APK Android,
  publiés en pré-release `nightly` à chaque `main` vert. Desktop livré
  (Windows validé mainteneur).
- **2026-10-02 (resynchronisation)** — P1.7 (palette par catégories,
  D109) constaté livré dans le code. P1.2 étapes 1–2 : bouton Flash
  masqué sur Android (`flash::offered()`), package APK `io.pnex.app`
  (`[bundle]` Dioxus.toml) ; reste le keystore de release.
- **2026-10-03 (harnais e2e)** — Suite Playwright `e2e/` (web en/fr +
  banc matériel C3 / ESP32-CAM) ; 23 correctifs produit issus des tests.
  Zones ouvertes consignées dans `docs/observations.md` O18–O25 ; décisions
  #12–#14 ajoutées.
