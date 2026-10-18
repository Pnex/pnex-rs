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
>
> **Revue du 2026-10-13** : resynchronisée avec `main` et les branches en
> cours. Les PRD du 2026-10-10/11 (géo phase F, pages, recherche IA,
> dossiers, empreinte vocale, ingestion média lots 0–6) ont du code sur
> `main` ; Media & Vision Studio (M1→M5) et le calendrier de maintenance
> (D183 + annexe B) sont mergés le même jour (section « Branches
> mergées »). Versions : **0.1.0 épinglée**
> (dernière pré-release `v0.1.0-beta.8`), la 0.2.0 annoncera ce qu'elle
> livre sans promettre de fonctionnalités à l'avance, toujours en beta.

## Positionnement : plateforme de données sur socle IoT (directive 2026-10-11)

PneX passe en mode **data platform**. Le produit n'est plus seulement une
plateforme IoT/SCADA : c'est une plateforme de données qui relie, dans
une même org, des sources hétérogènes et les rend exploitables ensemble.

- **Le cœur devient la donnée reliée** : noyau ontologique (objets, liens,
  temps — P2.14), séries temporelles O2, médias et transcriptions (P2.13,
  P2.15), documents et recherche IA (P2.18), pages (P2.17), couches géo
  (P2.16), dossiers et chronologie unifiée (P2.19). Le device devient une
  source de données parmi d'autres (imports, flux média, fichiers, API),
  la première et la mieux intégrée.
- **Le socle IoT reste entier, ambition et roadmap inchangées** :
  contrôle/commande (surfaces pilotables D123–D133, « une surface lit, un
  flow agit »), M2M résilient (M1–M3, `m2m-resilient.md`), firmware et
  OTA, identité device X.509 (D153–D158), régulation autonome (F3, D20),
  profils de sécurité (D148–D152), banc matériel (P0.1). Aucune entrée de
  ces volets n'est déclassée, reportée ni réduite par ce repositionnement.
- **Invariants conservés** : seul un flow déployé par l'humain agit sur le
  physique (D123, A4) ; l'assistant ne commande jamais un device ; la
  frontière de sécurité reste l'org puis la plateforme (D130).
- **Conséquence de tri** : à priorité égale, un chantier qui relie des
  données existantes (ontologie, chronologie, recherche) passe avant un
  nouveau pilier isolé — cohérent avec le gel des nouveaux piliers de la
  0.2.0 (`ontology.md` §9).

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

#### Geofencing : références et architecture pressentie (non développé, à trancher au PRD)

Rien n'est implémenté ; D38 (`device_positions` + séries O2) est le seul
socle existant. Références à étudier (idées reprises, aucun code copié) :

- **Tile38** (Go, MIT) — modèle d'événements :
  - événements typés `enter` / `exit` / `inside` / `outside` / `cross`
    (`cross` = traversée entre deux points jamais vus dedans, utile à
    faible fréquence GPS) ;
  - fences **statiques** et **roaming** (deux objets mobiles qui se
    rapprochent : camion ↔ dépôt mobile) ;
  - hooks déclaratifs : une fence = requête persistante (NEARBY / WITHIN
    / INTERSECTS) attachée à une sortie → chez PNEX, un nœud de flow.
- **Traccar** (Java, Apache-2.0) — modèle de domaine « suivi d'asset » :
  geofences en WKT (cercle, polygone, polyligne) rattachées à des devices
  ou groupes ; **polyligne + tolérance = corridor d'itinéraire** (alerte
  sortie de trajet) ; événements `geofenceEnter`/`geofenceExit` →
  notifications ; schéma et API REST comme plan du modèle Postgres.
- **ThingsBoard** (nœud action « GPS geofencing ») — anti-rebond :
  durée minimale dans la zone / hors zone avant de confirmer l'événement.
- **Home Assistant / OwnTracks** — UX grand public : zones cercle +
  rayon, point trop imprécis ignoré (`gps_accuracy_m`, D38), états de
  présence ; inspiration pour l'éditeur de zones sur /map.
- **PostGIS** — stockage, pas temps réel : `geography` + index GiST,
  `ST_Contains`, `ST_DWithin`, `ST_Buffer` (corridors). Jamais une requête
  SQL par point de télémétrie. Licence GPL-2 acceptée (2026-10-10) :
  extension de la base, programme séparé non modifié, PNEX ne lui parle
  qu'en SQL (aucun lien avec le code PNEX) ; si une image livrée l'embarque,
  notice + lien sources upstream (école ffmpeg). PostgreSQL est le seul
  moteur (décision #19) : pas de repli SQLite à prévoir.
- **Crates Rust** (évaluation in-process, licences à vérifier au LICENSE
  réel) : `geo` / `geo-types` (point-dans-polygone, distance, buffer),
  `rstar` (R-tree, fences candidates parmi des milliers), `h3o` (H3 pur
  Rust, pré-filtrage O(1) par cellules), `geozero` / `wkt` (échange WKT ↔
  GeoJSON MapLibre).

Architecture pressentie :

1. Fences définies en base et **versionnées comme les flows** ; une fence
   = un POI avec une géométrie et des règles d'événements (cohérent avec
   POI-first D35–D39 et les placements D43).
2. **Projection en mémoire** (R-tree ou H3) au déploiement.
3. **Machine à états par couple (device, fence)** : `outside → entering →
   inside → exiting`, avec dwell time et seuil de précision GPS. État à
   placer en Valkey (cluster D106 : un device peut changer de pod).
4. **Événements typés** (`enter`, `exit`, `cross`, `roaming`) vers le
   moteur de flows (nouveau nœud = `NodeDoc` + fiche assistant, D142) et
   l'alerting (nœud notify, journal `event_log`).

### Axe X — Extension navigateur (C1b → C3) : élevé

**Spec v2 proposée le 2026-10-18** (`extension-collector.md`, D254–D269) :
agents de veille (comptes X/TikTok, lives, pages HTML, registre des
vidéos, un agent par PC avec un onglet par plateforme, bail par cible). Historique v1 : la directive précisait la sémantique attendue : **collecte en temps réel et
refresh auto d'un site** — content-script avec polling configurable
(refresh auto) et détection d'événements quand le site s'y prête
(MutationObserver/réseau), transmission en WS device → pnex. Valider la
spec avec ces deux points, puis implémenter (MV3, consentement par
tâche).

## État par domaine (2026-09-23, revu 2026-10-01 et 2026-10-13)

| Domaine | État | Reste principal |
|---|---|---|
| **Scale horizontal** (D106–D108) | **Livré 2026-10-01** — flows shardés par org (contrôleur élu, fencing), bus device Valkey par pod, présence device en baux Valkey (Valkey obligatoire), audit multi-pods scale-a→e (quotas atomiques, versions optimistes, listes paginées SQL, single-flight) ; E2E 2 processus | **E2E multi-réplicas réel** (k8s : bascule contrôleur, perte de pod, rolling update) ; manifests k8s dans `pnex-deploy` ; `LAST_VALUES` encore local au pod (flow-engine §7) |
| **Palette de nœuds** | **Sections par catégorie livrées** (D109, P1.7) : 9 catégories en ordre fixe, recherche transverse | **P2.11** catalogue piloté par descripteurs (parité n8n) |
| **Firmware ESP générique** (F1/F2) | Consolidé — transport commun dans la lib `firmware/lib/pnex`, `Announce.caps`, StateReport par capacité ; flash F1/F2 validé réel (NodeMCU, ESP32-C3 0639ee4) | **F3 firmware `regulator`** (persist D45, buffer D46, boucle) — hardware requis ; e2e carte réelle Brick 0 jamais flashée |
| **Flow engine** | Consolidé — ETL full-Rust, device read/write (routage topic 90f23e0), calc, debug, http_fetch (C1a), nœud notify, cache live Valkey (b966c1a→e7257f8), **exclusivité write par pin** (8d60950), nœuds caméra/vision/mémoire/prédictifs, cluster D106 ; deploy de la version affichée + présence multi-utilisateurs dans l'éditeur (2578ffe) | Palette par capacité (ouverture §12, après P1.7 livré), E2E matériel du write/PWM, ack des commandes non persisté (brick0 §10) |
| **Fonctions custom** | Livré — JS (rquickjs) + Starlark (`_` privé, json en extension) | Limite acceptée : interrupt handler rquickjs ; payload remplacé (pas merge) — rien d'ouvert |
| **Notifications** | N1–N4 livrées (7 canaux, templates, anti-spam, canaux dédiés D61–D65), D66–D68 (trigger booléen), journal O2 D86, fan-out `/ws/notify` multi-pods | ~~N5 chiffrement AEAD des secrets~~ fermé par le coffre de secrets (2026-10-01) ; N6 points d'extension, canal gotify en réserve |
| **OTA** | Livré + E2E réel 8266 (2 cycles ~23 s, boucle 4003 anti-clone) ; OTA signée + anti-downgrade validées sur banc 8266 + ESP32-C6 (2026-10-09, SEC-18) | Politique de re-flash quand v2 du `.bin` (brick0 §10) |
| **Devices / pins** | Consolidé — CRUD, catalogue, quotas, pinout SVG v2, board variants, placements D43 | E2E matériel (write/PWM, fraîcheur read via Valkey) |
| **TFT / écrans** | V2 livré — barre dès boot, timeline OTA, panneau 21 pins (8e69519) | E2E matériel (tft_dev) |
| **Média (D21) + Take 360** | Couche 1 livrée ; Take 360 V2 serveur + V3 guidage device-validé + auto-cal focale | Capture portrait 2 anneaux, résolution 1080+, iOS/web, calibration par device |
| **Viz** (carte POI-first D35–D39, dashboards D40/D41, surfaces pilotables D123–D129) | Mergés ; dashboards mobile + contrôles d'org + nœud `control-source` livrés 2026-10-04 ; tables dormantes supprimées (migration de base D120, 2026-10-01) | Décisions §9 : style de tuiles configurable, diagramme de Mollier (log(p)-h, T-s et psychrométrique livrés dans le widget « Diagramme thermo ») ; E2E interactif + Android |
| **Studio** (tours + annotations) | Mergés (0aae2ad, 9e78241) | Maquette 3D, LRU panoramas, éditeur vue initiale, export offline ; annotations : reste couche ETL/device |
| **Assistant IA** (v1 + v2 D142–D145) | Livré — 35 outils fermés, base de connaissance embarquée + diagnostics, dashboards (D144), conversations privées en base + rétention (D145), LLM apporté par l'org (D119) ; finalisation i18n/audit 2026-10-07 (`ai-assistant.md` §10) | SEC-14 (filtrage d'URL du fournisseur, avec SEC-W3) |
| **Collection C1a** | http_fetch livré (2026-09-15) | — |
| **Extension navigateur (C1b)** | **Spec v2 proposée** 2026-10-18 (`extension-collector.md`, D254–D269 : agents de veille) — implémentation = jalon C3 | Validation v2 puis lots L0 → L6 |
| **Agent local (C2)** | **Livré** (2026-09-30) (`edge-agent.md`, D95–D99) : API locale libre, file disque, enrôlement une commande, Linux musl + Windows | Test Windows réel, macOS, pont WS devices LAN |
| **M2M résilient (M1–M3)** | PRD posé (`m2m-resilient.md`) | M1 POC zenoh-pico WiFi (zéro achat), M2 6LoWPAN à prototyper avant engagement |
| **Android** | Build + login PKCE E2E validé ; storage persistant ; scan LAN ; APK arm64 publié à chaque `main` vert (CI Apps, pré-release `nightly`, signé debug → sideload) ; bouton Flash masqué (`flash::offered()`), package `io.pnex.app` (`[bundle]` Dioxus.toml) | Clé de release à générer et APK release à valider (P1.2) ; libellé de l'app déjà « PNeX » |
| **Desktop natif** | **Livré 2026-10-01** — Linux x86_64 / aarch64 et Windows x86_64 (cross mingw) : login, flash USB par esptool embarqué ; validé mainteneur sur Linux et Windows ; publié à chaque `main` vert (CI Apps, pré-release `nightly`), smoke test de démarrage sur un runner Windows | Paquets installables (.deb/.msi), signature de code, macOS |
| **Schéma de base** (D120) | **Livré 2026-10-01** — une migration de base ; **SQLite retiré** le 2026-10-10 (518a069, décision #19), PostgreSQL 18 seul ; migrations 0.2 en `000002`–`000020` | Retrait du stockage `fs` (décision #25) |
| **Organisations (D42)** | Backend livré (labels, containment, edges) ; UI livrée (P1.1, 2026-10-09) : page Sites, fil d'Ariane Emplacement, labels D42 partout | Vue arbre « Organisation » dédiée |
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
| **Flux média entrants** (D159–D175) | **Code des lots 0 → 6 sur `main`** (2026-10-10, `media-ingest.md` §16–§18) : capture confinée + porteur `worker`, santé et alerte « flux muet », `media_source`, taxonomies + `topic_classify` (mots-clés), plages + import CSV/ICS, agrégats par plage + widget, VAD + diarisation locale, piste vidéo RTSP, boîtier de capture `pnex-media-capture` | Runs 24 h (sorties des lots 1 et 6), image ffmpeg avec décodeurs vidéo, RTSP sur vraie caméra, cross-builds agent, classifieur LLM + reclassement, vérification navigateur — P2.13 |
| **Ontologie / 0.2.0** (D176–D191) | **L0–L5 sur `main`** 2026-10-10 (`ontology.md` §10) : identité universelle, types et liens temporels, séries d'objets, requête et graphe, explorateur, dashboards de type, assistant, pack Maintenance. **Mergé le 2026-10-13** : D183 actions (flow déployé, `propose_action`) et calendrier de maintenance (récurrences, `/calendar`, pack Maintenance 1.2.0, SEC-35) | Éditeur d'actions, préconditions ; côté terrain : compteurs, OT = procédure (Pages L6), équipes, `.ics` ; mesures 100 k objets — P2.14 |
| **Media & Vision Studio** (D192–D210) | **M1 → M5 sur `main`** (2026-10-12/13, mergé le 13) : suivi et zones, légendes VLM + fusion sourcée, `text_decide` (Laya sur tract), Vision Lab (annotation, vidéos, interpolation, COCO), **entraînement sans Python** (tête YOLOX en Rust), file « à revoir », fichier comme source de flux, **rapports** = pages écrites par des nœuds (`llm_text`, `page_write`, `page_pdf`, pièce jointe e-mail) avec approbation humaine | INT8 après banc Pi 5 ; essais caméra et fournisseur réels — P2.15 |
| **Couches géographiques** | **Phase F sur `main`** 2026-10-10 (`geo-layers.md`) : fournisseurs géo d'org (fond, géocodage, reverse, routage) en coffre + proxy serveur, aucun fond semé, recherche d'adresse sur /map | Itinéraire entre POI, nœuds `geo_geocode`/`geo_route`, outils assistant ; phases A–E (PostGIS, layers MVT) après la 0.2.0 — P2.16 |
| **Pages collaboratives** | **L1–L2 sur `main`** 2026-10-10/11 (`pages.md` §15) : yrs serveur + Yjs client, édition live multi-pods, versions nommées, commentaires ancrés, import/export Markdown, éditeur façon Notion. 2026-10-13 : versions écrites par un flow, approbation, **export PDF** (krilla, part de L7) | Indexation des pages, import .docx, test multi-pods réel ; L3+ (mentions, vues de collection, procédures) — P2.17 |
| **Documents & recherche IA** | **P1 sur `main`** 2026-10-10 (`doc-search.md` §14) : extraction txt/md/docx/pdf/csv/xlsx/ods, `text_chunks` partagé, recherche lexicale, outils de l'assistant cités, **liste blanche stricte des téléversements** | Visionneuse PDF, outils testés avec un vrai LLM ; P2+ (pgvector, tableurs en SQL) après la 0.2.0 — P2.18 |
| **Dossiers (enquête, RCA)** | **Sur `main`** 2026-10-11/12 (`cases.md`, `cases-workspace.md`) : dossiers à membres et liens datés, ouverture depuis un événement, pack `investigation`, hypothèses, gel et export, **espace de travail plein écran** frise × carte × graphe (sigma, repli SVG), traces O2 `positions`. **Empreinte vocale** K19 variante A (`voice-id.md`) : profils consentis par Personne | Détecteurs (co-présence, zones, arrêts), enquête média K42–K47, avis juridique K19, ACL par dossier (V2, D188) — P2.19 |
| **Hub de kits** | **PRD gelé** 2026-10-10 (`hub.md`) — partage communautaire par Git + CI + index statique, slots et requirements | Rien avant la 0.2.0 — P3 |

## Branches mergées le 2026-10-13

`feat/gmao-plans` (D183 actions, étude et premier lot du calendrier GMAO,
SEC-35) puis `feat/media-vision-studio` (P2.15 M1 → M5, D192–D210,
migrations `000014`–`000020`, SEC-30 à SEC-34) sont mergées dans `main`
le 2026-10-13 après la suite de tests complète ; worktrees et branches
locales supprimés. Ordre de relecture de la branche vision :
`media-vision-studio-concepts.md` §8.

## P0 — Consolidation : fermer le livré non validé

Une grosse part du travail récent est livré mais attend le banc matériel —
la dette de validation s'accumule sur **une seule cause**, donc une passe
consolidée la dissipe d'un coup.

### P0.1 — Passe E2E matériel consolidée

Un banc (ESP32, NodeMCU, TFT, relais) déroule la liste :

> **Banc du 2026-10-14** (XIAO C3, NodeMCU V3 OLED, ESP32 38p TXD + TFT,
> ESP32-CAM-MB) : suites `@hardware` vertes (C3 7/7, NodeMCU 2/2, ESP32
> 3/3 avec OTA ajoutée, caméra 2/2) ; points 1, 3–7 validés. Reste le
> point 8 (firmware `regulator` pas encore écrit). Constats : O65, O66.

1. ~~**Write pin + PWM**~~ — validé le 2026-10-14 : flow déployé →
   `device_write` duty 100/75/50/25/0 sur la LED D4 du NodeMCU, paliers
   visibles, état confirmé par la carte et affiché sur l'OLED.
2. ~~**OTA sur ESP32 réel**~~ — validée le 2026-10-09 sur ESP32-C6
   (OTA signée, anti-downgrade ; SEC-18).
3. ~~**TFT**~~ — validé le 2026-10-14 (ESP32 38p TXD) : barre dès le boot,
   panneau de pins, page OTA ; la progression reste à 0 % pendant le
   téléchargement (O65, visuel).
4. ~~**Read**~~ — validé le 2026-10-14 : `device_read` A0 toutes les 2 s
   suit la carte (valeurs changeantes, pas de zéro inventé).
5. ~~**Écran debug local**~~ — validé le 2026-10-14 sur l'OLED soudé du
   NodeMCU (barre, pins, ADC et duty live). Brick 0 DoD §9 non rejouée.
6. ~~**Chaîne caméra complète**~~ — validée le 2026-10-14 : ESP32-CAM →
   `camera_source` → `vision_detect` (YOLOX nano, personne 87 %) →
   `event_log` + `json_split` → `pnex_notify` in-app envoyée. Le câblage
   via `calc` échoue en silence (O66).
7. ~~**Firmware custom**~~ — validé le 2026-10-14 sur C3 (e2e
   `custom-fw` : IDE → build → flash → métrique, générique restauré).
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

> **2026-10-14 — déroulé sur dev.pnex.io** (microk8s, 3 pods API+worker,
> `main-7c42677`, Valkey, ESP32-CAM réelle). **Validé** : bascule du
> contrôleur en arrêt propre (2,8 s, bail rendu) et en kill brutal
> (10 s, expiration du bail), tâches singleton reprises ailleurs, reprise
> de l'org d'un pod tué (flow redémarré en 10 s), device sur le pod A et
> flow sur le pod B (commandes publiées sur `pnex:devcmd:v1:<pod>`), device
> reconnecté sur un autre pod et commandes redirigées (~13 s), worker mort
> inéligible à 10 s et purgé à 10 min (`gc_ms`), zéro erreur HTTP sur les
> kills isolés. **Écart** : 2 timeouts de 5 s sur 1 365 requêtes pendant
> un `rollout restart` (O67, pas de `preStop`) → corrigé dans `pnex-deploy`
> (`preStop.sleep` 10 s) avec l'option `flowWorker.replicas` (pods
> flow-worker dédiés, API en `PNEX_FLOW_RUN_WORKER=false`,
> `horizontal-scaling.md` §1). **Après `helm upgrade`** (3 API + 2 flow-workers) :
> topologie séparée validée (contrôleur sur un pod API, flow sur un
> flow-worker, caméra sur un pod API, commandes via le bus) ; rolling update
> rejoué : 1 timeout sur 743, 50 s après la fin du roulement, imputé au
> contrôleur ingress microk8s en CrashLoopBackOff sur un nœud (hors PneX).

## P1 — Compléter les troncs inachevés

### P1.1 — Organisations : UI arbre (D42)

Le backend (labels GIN, containment, edges) est livré ;
il manque l'UI (arbre des couches org). Sans elle, la couche org n'existe
pas pour l'utilisateur — c'est le plus grand trou user-facing du socle.

> **2026-10-09** : filtre `label=` (effectif, héritage compris) ajouté aux
> listes devices, dashboards et visites (`labels::list_filter`, test
> `resources.rs::label_filter_on_devices_and_dashboards_lists`) ; l'UI
> n'expose encore les labels que sur les médias. **À trancher avant
> l'UI** : la fiche device affiche déjà une carte « Labels » qui édite les
> *métadonnées* libres (`metadata`), distincte des labels D42 — fusionner
> (les métadonnées deviennent des labels D42) ou renommer l'une des deux ;
> et où vit l'arbre (page dédiée « Organisation » vs extension du tiroir
> POI déjà en arbre mixte).
>
> **2026-10-09 (tranché, livré non commité)** : un seul arbre, celui des
> lieux, deux entrées — la Carte et la page **Sites** (`/sites`, menu
> Visualisation : liste pleine page des repères, même tiroir que la carte,
> « Vue carte » / « Vue liste » pour basculer) ; fil d'Ariane
> **Emplacement** « Site › dossier › dossier » dans les fiches device et
> média et dans les éditeurs dashboard / visite, chaque étape ouvre le site
> (`GET /resources/{kind}/{id}/location`, test
> `location_breadcrumb_follows_folders_and_site_links`). Pas de page « arbre
> abstrait » (« Organisations » = espaces de travail, nom déjà pris).
>
> **2026-10-09 (tranché, décision utilisateur)** : les labels sont **un seul
> mécanisme global**, les labels D42. La carte « Labels » de la fiche device
> édite désormais les labels D42 (`LabelsEditor`), le wizard d'enregistrement
> les collecte (composant partagé `LabelChipsInput`, écrits via
> `PUT /resources/device/{id}/labels` juste après la création) ; l'UI
> n'envoie plus `metadata` (champ/API conservés côté backend, legacy masqué,
> pas de migration ; `KvPillsEditor` supprimé). Même éditeur partout :
> bouton « Labels » de la barre des éditeurs flow / dashboard / visite
> (`LabelsButton`), section du tiroir POI (`map_pin`), icône étiquette des
> dossiers de l'arbre POI (`folder`). Filtre label (`SearchInput`) sur les
> listes devices, dashboards, visites et flows ; `label=` ajouté à
> `GET /flows` (test `label_filter_on_flows_list`). Pas de chips de labels
> dans les lignes de liste (aucun endpoint batch hors médias). La page
> Événements n'affiche plus de JSON brut (liste clé/valeur aplatie). Reste
> ouvert : la vue arbre « Organisation » dédiée.

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
3. ~~Keystore de release~~ — plomberie livrée le 2026-10-09 :
   `task build:frontend:android:release` (type `release` non debuggable,
   signé par la clé lue dans l'environnement, R8 désactivé car il casse
   les classes atteintes par JNI, `versionCode` dérivé du dernier tag
   `v*` : `0.1.0-beta.7` → 100207) ; job CI `android` signé dès que les
   secrets `ANDROID_KEYSTORE_B64`, `ANDROID_KEYSTORE_PASSWORD`,
   `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` existent (repli debug
   sinon). Libellé de l'app déjà « PNeX ». **Reste** : générer la clé de
   release (à conserver hors du dépôt, sauvegardée : la perdre interdit
   toute mise à jour sur le Play Store), la poser en secrets, valider
   l'APK release sur téléphone (les testeurs désinstallent l'APK debug :
   signature différente).

### P1.3 — F3 : firmware `regulator` (D20)

Prouve l'autonomie du bord : persistence Config (D45), buffering D46,
boucle de régulation, golden vectors déjà en CI (`pnex-core-cpp` ne peut
pas dévier sans casser la CI). Hardware requis — se combine naturellement
avec la passe P0.1 (même banc).

### P1.4 — Annotations : couche ETL/device

> **✅ Couvert par D147** (2026-10-07, `annotations.md` §13), constaté le
> 2026-10-15 : les annotations sont des mini-tableaux de bord ancrés sur
> le média (valeur, sparkline, jauge, indicateur sur une série, contrôles
> d'org). L'écriture directe d'un device depuis une annotation reste
> exclue (D123 : une surface lit ou écrit un contrôle, un flow agit).
> Section conservée pour l'historique.

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
| Code | function (red retiré le 2026-10-14) |
| Stockage & séries | metric, memory-write, memory-read, video-record, event-log |
| IA & prédictif | vision-detect, anomaly, forecast |
| Intégrations | notify, http-fetch |
| Debug | debug |

Ordre des catégories = parcours d'un flow (source → traitement → sortie).
La couleur des nœuds au canevas reste par kind dans cette tranche.

### P1.8 — Clarté de l'UI : vocabulaire, étiquettes, annotations, lecteur, versions (ajout 2026-10-15)

Décisions D218–D224 (`design-system.md` §7) et plan en quatre phases
(§8) : (1) lexique unique et étiquettes sur tout objet listable, dans la
recherche globale ; (2) annotations recentrées sur les médias visuels,
entrée « Annoter » depuis l'objet ; (3) un seul lecteur temporel
(transcriptions, segments, plages, fenêtres de temps) et un historique de
versions commun avec frise et trois verbes ; (4) une coquille d'éditeur,
une forme d'ouverture par usage, primitives sans copie.

## P2 — Nouveaux chantiers prêts (spec écrite)

### P2.1 — Extension navigateur (C1b → jalon C3) — **élevé (directive produit)**

**v2 du 2026-10-18** (`extension-collector.md`, D254–D269, proposée) :
agents navigateur (un par PC, un onglet par plateforme), missions `collect_value` /
`collect_page` / `follow_account` / `watch_live`, registre des vidéos,
ticket device → `/ws/extension` + upload HTTP, crate wasm
`pnex-extension`, Chrome puis Firefox ; lots L0 → L6. Ancien libellé
(v1) : implémentation de `extension-collector.md` amendée par la directive :
WS device unique, crypto WASM pnex-core, entité `extension_rules`,
`ServerMsg::RulesConfig`, OIDC plan gestion / device token plan données,
connect-on-wake, cast ≠ apply — plus **refresh auto configurable** et
**collecte temps réel** (content-script, MutationObserver quand possible).
MV3 + consentement par tâche. Pré-requis : validation de la spec amendée
(P1.5).

**État 2026-10-18** : v2 codée (L0–L5, X / YouTube / TikTok, historique,
sous-titres, enregistrement par parties, lives, Firefox, mode dédié),
revue de sécurité faite ; campagne de test à mener (O94). **À planifier**
(O93) : L7 recherche sur les plateformes filtrée par le registre,
adaptateur Facebook, déduplication par contenu (même vidéo republiée),
publication Chrome Web Store et signature AMO. **Phase à part**
« rétention et limites d'usage » (O91) : revoit ensemble D161, D268, O2,
assistant, builds, quotas par tier et plafonds ; migration
`media_versions.storage_key` nullable à prévoir.

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

### P2.10 — Firmware custom : IDE intégré (`custom-firmware.md`, D87–D94) — **en cours** (L1/L3/L4/L5 livrés, L2 partiel)

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
plans ⇒ un endpoint S3 (RustFS par défaut, jamais une
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

### P2.12 — Dashboards « Maison » : palette domotique (ajout 2026-10-04) — **livré 2026-10-04** (lots A → E)

Plan validé le 2026-10-04 (`home-dashboards.md`, D134–D141) : palette
catégorisée, règles d'état, icônes maison, primitives de contrôle
`select`/`stepper`/`command`/`color`, cartes composées par domaine
(éclairage, climat, ouvrants, sécurité, énergie, environnement, scènes,
appareils), nœud `pnex-weather` (actuel / 7 jours / 48 h → mémoire ou
O2), pages + pièces + chips en mobile, modèles Maison/Énergie/Sécurité/
Jardin. Lots A → E.

**Reportés (consignés, `home-dashboards.md` §6)** :
- **REP-1 — Mode sombre global** : chantier transverse sur toutes les
  pages (jetons de couleur, variantes sombres des cartes et symboles).
- **REP-2 — Code PIN serrure/alarme** : écriture protégée vérifiée côté
  serveur ; d'ici là, simple confirmation.

### P2.13 — Ingestion de flux média : transcription, plages (ajout 2026-10-09) — **code des lots 0 → 6 sur `main`** (2026-10-10), runs 24 h et vérifications réelles à faire

PRD `media-ingest.md` (D159–D175) ; code des lots 0 → 6 sur `main`. Capter
des flux continus (Icecast, HLS, DASH, RTSP, DVB-T via Tvheadend) en
**audio seul**, les transcrire en quasi temps réel sur un worker GPU
(job Loco, tag `asr` réglable), ranger le temps en **plages** annoncées/recalées
et lire les chiffres dans PNEX (pas de publication). Premier cas d'usage :
couverture des sujets par chaîne (radios et TNT publiques), agrégats
seulement. Invariants : le runtime de flows ne voit que du texte
(`media_source`), audio éphémère par défaut, aucune identification
vocale, egress filtré, garde-fous juridiques encodés (TDM, rien ne sort
de l'org). Lots 0 → 6 :

0. **POC ASR** : crate `pnex-asr` (trait `Transcriber`), sherpa-onnx vs
   whisper.cpp sur 1 h de radio annotée (WER, noms propres, RTF GPU/Pi).
1. **Capture + transcription** (D159–D162, D165–D167) : France Inter
   24 h, couverture ≥ 99 %, zéro audio résiduel.
2. **Flows + séries** (D163, D168, D171) : dashboard « mentions par
   heure » sur 3 flux.
3. **Plages + métadonnées** (D169, D170) : stats par émission sur une
   semaine — dépend du parseur XML de P2.11.
4. **Consultation** (D173) : stats par plage dans les dashboards PNEX et
   `/streams` — pas de publication de rapport (décision user 2026-10-09),
   studio de rapports en tranche ultérieure.
5. **Diarisation** (D166) : temps de parole par plage.
6. **Boîtier de capture** (D160, D175) : agent edge `media_capture` sur
   canal device mTLS + Noise, caméras IP sur le bus.

Dépendance : fabric de workers (P2.7) pour le worker GPU distant,
contournable au lot 1 par un process `--worker` joint en mesh.
Ordre (décision #17, tranchée 2026-10-09) : **média d'abord**. Les types
système restent dans leurs tables (D177) : `media_stream` et
`time_range` seront enregistrés par adaptateur à L1, sans migration ;
seules les mentions nominatives (lot 2) attendent le pack « Couverture
médiatique ».
Décision #16.

### P2.14 — 0.2.0 : noyau ontologique (objets, liens, temps, actions) (ajout 2026-10-09) — **L0–L5 livrés 2026-10-10, D183 + calendrier livrés le 2026-10-13**

PRD `ontology.md` (D176–D191), socle de la **version 0.2.0** ; L0–L5
sur `main`. Généralise la couche Resource D42 : types d'objets
définis par l'utilisateur (données versionnées, `KindSpec` dérivé),
propriétés typées dont temporelles (désignent une série ou un stream
O2, ne copient rien), liens typés à validité temporelle, **le temps
appartient aux objets** (un capteur remplacé ne casse plus la courbe de
la pompe), provenance sur chaque fait, API de requête JSON,
explorateur générique, packs métier. Les types système
restent dans leurs tables (adaptateurs) ; migration 0.1 → 0.2 **non
destructive**.

| Lot | Contenu | Sortie |
|---|---|---|
| L0 | Spike : types en données dérivant le `KindSpec` | registre généré == registre codé |
| L1 | D176–D178 : types, objets, propriétés scalaires, YAML | « Pompe » + 100 objets |
| L2 | D179–D180 : types de liens, validité temporelle, migration `placed_on`/`placed_at` | requête `as_of` correcte |
| L3 | D178 temporel + D181 : liaisons device → objet | remplacement de capteur sans rupture |
| L4 | D185–D187 : requête, explorateur, dashboards de type | dashboard de type Pompe |
| L5 | D184, D189, D191 : provenance, assistant, migration | base 0.1 réelle migrée |
| 0.2.0 | L0–L5 + pack « Maintenance augmentée » livrés sur `main` | la release annonce ce qu'elle livre, sans promesse à l'avance (revue du 2026-10-13) |
| 0.3 | ~~D183 actions~~ (remontées dans la 0.2.0, livrées le 2026-10-13), packs Maison et Couverture médiatique, ACL par objet | — |

**Calendrier de maintenance** (annexe B de `ontology.md`, premier lot
2026-10-13) : propriété `recurrence` (tous les N jours, semaines, mois,
ans ; délai de préparation), occurrences matérialisées en plages de
l'objet sur 90 jours, jamais réécrites une fois touchées par une
personne ; pack Maintenance 1.2.0 (`maintenance_plan`, lien
`maintains`) ; page `/calendar` ; outils de l'assistant en lecture et
proposition. Écart assumé : pas de RRULE tant qu'aucun plan ne demande
de jour de semaine ou d'heure.

**Annexe stockage graphe** (2026-10-10, `ontology.md` annexe) :
ontologie optionnelle ; **identité universelle** (une ligne `objects` UUID
+ temps de validité pour tout objet, contenu natif en place, liens à FK
réelles — amende D177, tranche Q1) ; interface graphe unique dans
`pnex-core` (jointures + CTE + `petgraph`, pas de base graphe) ; API
bornée à 4 sauts, illimité en interne ; PostgreSQL 18 maintenu
(SQL/PGQ absent de PG 19) ; vue graphe Dioxus et verrou par type ajoutés au L4.

**Risque de dispersion acté** : pendant la 0.2.0, aucun nouveau pilier
fonctionnel ne démarre (correctifs et finition seulement) — à arbitrer
avec P2.13 et P2.1. Décision #17 — **exception P2.13** (2026-10-09) : le
média démarre avant la 0.2.0 (`media-ingest.md`, en-tête).

### P2.15 — Media & Vision Studio : tracking, zones, Vision Lab, description (ajout 2026-10-10) — **M1 → M5 livrés (2026-10-12/13)**

**État 2026-10-13** : Q1–Q15 tranchées, D192–D210 livrés (mergés dans `main`
le même jour). Écarts au PRD initial, décidés
en route : **aucun Python nulle part** (entraînement = réapprentissage
de la tête YOLOX en Rust, pas de job `train_detector` en image Python) ;
M5 n'est **pas un studio de rapports** mais une page écrite par des
nœuds génériques (`inject` → données → `llm_text` → `page_write` →
`page_pdf` → `pnex_notify` avec pièce jointe), approbation humaine par
défaut des versions IA (D210), PDF par krilla (Typst écarté). INT8
évalué, non livré. Détail et ordre de relecture :
`media-vision-studio-concepts.md`.

PRD `media-vision-studio.md`. Texte d'origine (2026-10-10) : étend la caméra
(D73–D105) et l'ingestion média (P2.13) sans les refaire : nœuds
`vision_track` (ByteTrack en Rust), `vision_zone` (polygones sur
l'image), `vision_caption` (VLM de l'org, D119), `media_fuse`
(description horodatée sourcée, D184) ; **Vision Lab** = volet vision du
Model Lab de l'axe C : annotation dans PNEX, datasets versionnés, job
`train_detector` dans la queue Loco (seul composant Python, image YOLOX
figée hors image serveur), ONNX publié dans le registre existant (D81,
check D100). Phases M0 (= P2.13 lots 1 et 6) → M5 (prévu comme studio de rapports,
qui rouvre D173/D174/D3 ; livré autrement, D207–D210, voir l'état
ci-dessus). Répond à la décision #7 (fine-tuning
détecteurs). Dépend de P2.7 (GPU) et de P2.13 ; cible d'origine après la 0.2.0,
dépassée : gel levé pour ce pilier, mergé dans `main` le 2026-10-13.
Décision #18.

### P2.16 — Couches géographiques et fournisseurs géo (ajout 2026-10-10) — **phase F sur `main` (2026-10-10)**, A–E après la 0.2.0

PRD `geo-layers.md` (phase F sur `main`), décisions L1–L28
provisoires (numéros D à la validation). Extension de la base map, pas un
nouveau pilier : **PostGIS** en extension du Postgres existant (image
multi-arch, arm64 obligatoire), fichier géo = média (kinds `vector` /
`raster`), import en job worker (`import_geo` : sniff → mapping → staging
→ publication sans coupure, wipe ou upsert par clé), tuiles MVT servies
par Loco avec génération dans l'URL, cache Valkey obligatoire, seed
PMTiles opt-in, GC des dérivés. **Martin garde le fond de carte.**
Phase F : fournisseurs géo d'org (fond, geocoding, reverse, routage,
école `llm_providers`), secrets en coffre, proxy serveur, rate limit —
**retire l'URL de fond codée en dur** (D27). Phases A → F. Partage
l'image Postgres étendue avec P2.18 (pgvector). **Phase F autorisée
avant la 0.2.0 (consolidation), A–E après.** Décision #20.

### P2.17 — Pages : documents collaboratifs reliés à l'ontologie (ajout 2026-10-10) — **L1–L2 sur `main` (2026-10-11)**, export PDF (2026-10-13)

PRD `pages.md` (L1–L2 sur `main`), décisions P1–P16
provisoires. Page = type système `doc` de l'ontologie ; contenu CRDT
(Loro ou yrs, spike L0) répliqué par le serveur, WS `/ws/doc/{id}` +
fan-out Valkey multi-pods ; éditeur ProseMirror en bundle IIFE
(`window.pnexDoc`, école `pnexMap`), blocs PNEX rendus par Dioxus ;
mentions = liens `mentions` (rétroliens), vues de collection éditables
via le service ontologique, tableau local IronCalc (formules en lecture
seule), mesures avec tolérance, procédures et workflow de page, bloc
carte live/figé, versions nommées. Lots L0 → L7. **L1–L2 en exception
au gel (tranché 2026-10-10), L3+ après la 0.2.0.** Décision #21.

### P2.18 — Documents & recherche IA (RAG hybride) (ajout 2026-10-10) — **P1 sur `main` (2026-10-10)**

PRD `doc-search.md` (v0.2 relue), P1 sur `main`. Fichiers
du quotidien (txt, md, docx, pdf, csv, xlsx/ods) déposés dans la
médiathèque, extraits en job, découpés et indexés dans Postgres
(`tsvector` simple + french, trigrammes pour les codes défaut, pgvector
+ RRF en phase 2) ; outils de l'assistant en lecture seule avec
citations (doc + page) ; tableurs interrogés en SQL (Parquet +
DataFusion), pas en RAG. Embeddings locaux (e5-small) ou fournisseur
d'org. Phases P1 → P5 (P4 = liens ontologie, 0.2.0). Extracteurs et
index partagés avec P2.17. **P1 seul en exception au gel (tranché
2026-10-10), P2+ après la 0.2.0.** Décision #22.

### P2.19 — Dossiers : enquêter sur données × temps × ontologie × acteurs (ajout 2026-10-11) — **C0–C7 et espace de travail sur `main` (2026-10-12)**

PRD `cases.md` (C0–C7 sur `main`), décisions K1–K19
provisoires (numéros D à la validation). Un **dossier** (`case`, type
système) est une lentille sur l'ontologie : il référence sans copier
(lien `in_case` temporel, rôle), porte une fenêtre d'intérêt et une
question. Même notion pour la RCA industrielle (ouverture depuis une
alerte), l'enquêteur « données seules » (imports CSV/GPS), le vidéaste
et le journaliste. Noyau : `case`, `in_case`, **chronologie unifiée**
(`/timeline` : événements, liens, `object_changes`, journal D86,
caméra, transcriptions, plages, séries) et type `fuzzy_time` ; métier
dans le pack `investigation` (personnes, organisations, événements,
pièces, affirmations sourcées, hypothèses). Confinement V1 = org
dédiée (aucune frontière neuve), ACL par dossier en V2 avec D188.
Apparitions média (K17) : qui est passé dans quelle émission, à
l'heure de diffusion (live ou différé), rattaché par grille,
transcription ou mention, jamais par la voix ; collecte conçue par
l'assistant, exécutée par des flows déployés par l'humain (K18).
Lots C0 → C8. Proposé à l'origine : C0–C1 comme validation de la 0.2.0
(première application du noyau) ; C0–C7 sont sur `main` depuis le
2026-10-12. Décision #24.

**Refonte de l'écran (2026-10-12)** — PRD `cases-workspace.md` v0.2
(K20–K41) : l'écran formulaire devient un espace de travail frise ×
carte × graphe. L'événement daté entre dans le noyau (`event`, attache
`involves` à rôle fermé), les membres se déduisent des attaches, les
traces sont lues depuis un stream O2 `positions` (devices et imports
GPX/CSV), et des détecteurs Rust (co-présence, zones, arrêts, fin de
signal) **proposent** sans jamais ajouter. Deux tables seulement
(`case_proposals`, `case_snapshots`). Démarche imposée : benchmark
(**fait**, `docs/bench/dossier-enquete.md`) → design (maquettes aux
jetons PNEX) → modèle → code, lots L0–L6.

### P2.20 — Calendrier GMAO (ajout 2026-10-13) — **étude faite, premier lot de code sur `main` (2026-10-13)**

Annexe B de `ontology.md`. Étude clean-room d'un corpus open source (Atlas
CMMS, Odoo Maintenance, ERPNext, openMAINT ; EventCalendar, FullCalendar,
Frappe Gantt ; crates `rrule`, `icalendar`, RustiCal ; contraintes
Timefold) sur 6 scénarios communs, fiches et matrice dans
`research/gmao-calendar/` (captures non versionnées, aucun extrait de code
tiers). Hypothèse directrice : la GMAO est un pack (D190) qui réutilise
l'existant (plan sur tout objet y compris un device seul, occurrence =
plage D182, OT = procédure instanciée `pages.md` P9, action au début
d'occurrence = D183, compteur = série O2) ; seule la récurrence
(`pnex-core`, wasm32) est neuve dans le noyau. **Premier lot codé le
2026-10-13** (B13, voir P2.14), avant la relecture complète de
`modele-cible.md`. Reste : compteurs, OT = procédure, équipes et vue de
charge, Gantt d'arrêt, flux `.ics`. Décision #26.

### P2.21 — Éditeur de flows lisible : groupes de nœuds et valeurs au canevas (ajout 2026-10-15) — **à cadrer**

Suite d'O70 : rendre les flows lisibles par un non-développeur, même avec
beaucoup de nœuds. Déjà livré : `inject` typé, type des ports et des fils,
rejets visibles (O66, O69), sélecteur d'appareils complet (O68), valeur
déclarée par sortie cochée du `Device (écriture)`. Reste, gros chantier :

- **Groupes de nœuds** : cadre nommé et repliable autour d'un ensemble de
  nœuds (lisibilité « façon classes »), avec le zoom et le « tout afficher »
  existants (O51).
- **Valeurs déclarées affichées sur le canevas** : l'ancre de pin les porte
  mal (même ligne que le titre) ; il faut une place dédiée sur le nœud.
- Toute évolution passe par la `NodeDoc` et l'assistant (D142–D145).

### P2.22 — Création assistée : noyau et pack (ajout 2026-10-16) — **PRD v0.3, D225–D237 validées**

Spec : `assisted-creation.md`. Règle tirée des dossiers (K55) : aucun type nouveau quand un type existant plus un rôle suffit. Une production est un **espace de travail** comme le dossier (frise, Histoire, graphe, carte servent tels quels), une scène est un **événement**, l'ordre de présentation est un **lien ordonné** (vue Plan). Noyau : espace de travail générique, rôles d'attachement déclarés par un pack, temps relatif, liens ordonnés, assets sur tout objet (`depicts`) + visionneuse 3D, masquage par défaut des données personnelles à l'export, « contenu généré » (signature C2PA reportée), exports OTIO/FCPXML et vidéo des vues. Pack `creation` : production, partie, personnage, cadrage ; profils Enquête, Vidéo, Fiction.

Premiers lots : C1 (assets + visionneuse 3D) et C2 (espace de travail générique + vue Plan), utiles hors création. Lexique arbitré : Production, Partie, Séquence, Personnage, Cadrage (Projet, Scène et Plan étaient déjà pris).

### P2.23 — Observabilité IT : ingestion des protocoles classiques (ajout 2026-10-17) — **pas démarré, à cadrer**

Directive : PneX porte **toute donnée**, pas seulement le monde physique.
Logs, traces et métriques applicatifs entrent dans la même org que la
télémétrie des appareils et se relient aux mêmes objets (un serveur, un
service, une application = un objet de l'ontologie). Rien n'est en place
aujourd'hui ; aucune annonce « disponible » avant le code.

- **Protocoles visés** (les classiques, aucun format maison) :
  - **OpenTelemetry OTLP** (gRPC et HTTP) : logs, traces, métriques ;
  - **Prometheus** : scrape de cibles `/metrics`, réception `remote_write`,
    et exposition `/metrics` de PneX lui-même ;
  - **syslog** (RFC 5424 / 3164, UDP, TCP, TLS) ;
  - **Loki push API**, **InfluxDB line protocol**, **StatsD** ;
  - collecteurs du marché (OpenTelemetry Collector, Fluent Bit, Vector)
    branchés sur l'un des points ci-dessus, sans agent propre.
- **Piste d'architecture** (à trancher au PRD) : O2 parle déjà nativement
  plusieurs de ces protocoles, mais il n'est **jamais exposé** ; PneX reçoit,
  authentifie par jeton d'ingestion à portée org (org depuis le principal,
  R1 ; aucun identifiant O2 visible, R4/R16), puis écrit dans le stream O2
  de l'org. L'agent edge peut relayer (tampon disque) pour les sites isolés.
- **Usage dans le produit** : traces et logs consultables depuis l'objet,
  sources de flows (alerte sur taux d'erreur, seuil de métrique), corrélation
  dans la chronologie des dossiers (incident applicatif × événement terrain).
- **Garde-fous** : quotas et rétention par org (D72), limites de débit,
  `cargo deny` sur toute dépendance de décodage (OTLP protobuf, snappy pour
  `remote_write`) ; fiche assistant (D142) et clés fluent dès la première UI.

### P2.24 — Sécurité logicielle et qualité du code : un seul endroit (ajout 2026-10-17) — **pas démarré, à cadrer**

Constat : SBOM, scans de vulnérabilités, analyse statique, secrets, licences,
couverture et qualité vivent dans des outils séparés, chacun avec sa base et
sa vue. PneX les rassemble dans l'org, reliés aux objets qu'ils décrivent
(application, version, image, firmware, appareil qui l'exécute), avec le
temps, les flows, les dossiers et les pages déjà en place.

- **Principe : ingérer, ne pas réécrire les scanners.** PneX reçoit les
  sorties des outils existants dans leurs formats standard :
  - **SBOM** CycloneDX et SPDX ; **VEX** (CycloneDX VEX, OpenVEX), avis CSAF ;
  - **SARIF** : analyse statique, linters, secrets, IaC, scans de conteneurs
    (Semgrep, gitleaks, Trivy, Grype, clippy, ESLint…) ;
  - résultats `cargo audit` / `cargo deny`, osv-scanner, scans dynamiques ;
  - qualité : couverture (lcov, Cobertura), rapports de tests (JUnit XML),
    évolution de la couverture et tests instables d'un run à l'autre ;
  - **pipelines CI/CD** : état des runs et des jobs, builds, durées, artefacts
    et déploiements, reçus par webhook ou API (GitHub Actions, GitLab CI,
    Forgejo/Gitea Actions, Jenkins) ; indicateurs de livraison (fréquence de
    déploiement, délai, taux d'échec, temps de rétablissement).
  - Piste : une API d'envoi de SBOM compatible avec celle de Dependency-Track,
    pour brancher les intégrations CI existantes sans changement.
- **Ontologie (pack `software`)** : Application, Version, Composant (purl),
  Dépôt, Image, Vulnérabilité (CVE, GHSA), Constat, Licence ; liens datés
  « version contient composant », « composant affecté par », « image déployée
  sur ». **Pont avec le socle IoT** : chaque build firmware enregistré produit
  son SBOM, chaque appareil sait ce qu'il exécute → exposition d'une flotte à
  une CVE en une requête (exigence CRA).
- **Sources de vulnérabilités** : OSV, NVD, GHSA, EPSS, catalogue KEV, via le
  filtre de sortie (R8) ; **miroir hors ligne** importable pour les
  installations isolées.
- **Usage** : vue portefeuille, politiques (licences autorisées, seuils
  CVSS/EPSS, KEV bloquant), tri des constats avec statut et justification VEX,
  évolution dans le temps, alertes par flow, dossier d'incident, rapport
  d'audit ou de conformité en page PDF, l'assistant lit et résume (lecture
  seule).
- **Garde-fous** : envoi par jeton à portée org (même mécanisme que P2.23,
  R1) ; fichiers non fiables → tailles bornées, parseurs sans exécution ;
  constats et vulnérabilités = données sensibles → dépend du contrôle
  d'accès par objet (D188) ou d'une org dédiée ; dépendances permissives et
  `cargo deny`.

### Outils externes évalués (ajout 2026-10-11) — **inscrits, rien d'implémenté**

Contraintes de tri : workspace Cargo unique, cible Raspberry Pi (pas de
plafond mémoire strict : ~2 Go au repos avec des pics est acceptable, la
charge se gère au scheduling), PNEX sous MIT et visée d'une fondation (CNCF/Apache) →
dépendance du cœur = licence **OSI, permissive de préférence** ; une seule
UI, tout moteur tiers reste headless derrière elle.

**À intégrer au cœur**

| Outil | Licence | Rôle | Pilier | Statut / jalon | Contraintes |
|---|---|---|---|---|---|
| **anydoc** (firecrawl/anydoc) | MIT, pur Rust | docx/pptx/xlsx/odt/rtf/epub/csv/pdf → Markdown | Médiathèque + RAG (P2.18), agent IA de debugging | À intégrer, avec le chantier RAG (P2.18) | PDF scannés non gérés → OCR local (`ocrs` ou Tesseract) ; **jamais** l'offre hébergée Firecrawl |
| **fframes** | MIT, Rust | Rendu vidéo MP4 depuis un arbre SVG par frame | Export vidéo du replay temporel (synoptique/dashboard sur une plage) : RCA, dossiers (P2.19), studio de rapports | À intégrer **après la 0.2.0** (dépend du temps) ; **valider avant engagement** le rendu SVG serveur des synoptiques partagé dans `pnex-core` | Rendu CPU → queue de workers (école build firmware) ; FFmpeg **sans** `libav-agree-gpl` / `-nonfree` / `-version3`, codecs compatibles MIT |

**Références de design (inspiration, aucune dépendance)**

| Outil | Pilier | Note |
|---|---|---|
| Open-notebook | Dossiers / espace projet (P2.19) | Modèle sources + chat contextuel + recherche |
| OpenFlowKit, DeepDiagram | Canevas d'enquête, éditeur type Notion (P2.17, P2.19) | Stack React → refaire en Dioxus |
| Whitebophir | Canevas collaboratif | Licence copyleft à vérifier |
| Kaneo | GMAO (ordres de travail) | Référence UI kanban |
| Aria-Icons | Symboles de l'éditeur de synoptiques | Licence variable par collection → filtrer (règle « assets tiers : jamais de copie ») |
| Kiln | Assets 3D (GLB) d'équipements pour synoptiques / visites 3D | Outil de production d'assets, pas runtime |

**Sidecars optionnels (hors cœur, profil docker compose)**

- **Stirling-PDF** : fusion, caviardage PDF pour dossiers/GMAO, appelé en
  REST depuis un nœud de flow. Lourd (Java + LibreOffice) ; **cœur MIT
  seulement**, dossiers `proprietary`, `saas`, `engine` exclus.
- **Steel-browser / Figranium** : nœud d'ingestion web ou outil de
  navigation de l'assistant ; navigateur headless → hors cœur.
- ~~**Typst**~~ (Apache-2.0) : écarté le 2026-10-13 pour les PDF de
  pages (128 crates dont une non maintenue, interpréteur WASM inutile) ;
  **krilla** + harfrust retenus (crate `pnex-pdf`, D209).

**Tutoriels générés : dans `pnex-website`** (repo privé, génération
interne ; pas de repo dédié ni d'open source) — étend le scénario de
captures existant pour la communauté Instructables / Medium. À chaque
release : instance de démo (compose), pilotage de l'UI, capture, montage,
publication. Seul couplage avec pnex-rs : API publique + compose. Outils
à intégrer : Openscreen (capture), fframes (montage
as-code), archify (diagrammes d'archi animés), Anatomy (vues isométriques
des cartes), Kiln (rendus 3D), Stickman Video Director (modèle cloud →
marketing uniquement).

**Écartés** : OmniParse (Python, GPU, couvert par anydoc), Lorien,
ArtCraft, Zennotes, **Reticulum** (licence non OSI ; le transport M2M
reste celui de `m2m-resilient.md` : zenoh-pico + OpenThread, tout open
source — pas de discussion).

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
- **Profils de sécurité, du maker au militaire** (`security-tiers.md`,
  D148–D152, ajout 2026-10-08) : exigences les plus élevées tracées
  (IEC 62443 SL1 → SL4, CRA, ANSSI), activables par flags
  (`open` par défaut, `industrial`, `critical`, `sovereign`). Vagues :
  V1 correctifs protocole pour tous (SEC-17 à SEC-19 : frames AEAD +
  anti-rejeu, OTA signée, anti-downgrade logiciel) ; V2–V3 profils
  industriel et critique, logiciels ; **V4 eFuses seulement après 1 à 2
  ans de validation communautaire du firmware** ; V5 souverain sur
  demande client. **V1 livrée le 2026-10-09** (Noise AEAD + anti-rejeu,
  OTA signée Ed25519 + anti-downgrade, TLS obligatoire, X.509/mTLS par
  device, secret d'edge — D153–D158, banc 8266 + C6) ; V2–V5 non
  commencées. Décision #15.
- **Apache AGE** (`ontology.md` annexe A4, ajout 2026-10-10) :
  projection graphe en lecture par triggers, optionnelle, Postgres
  restant la source de vérité. Déclencheur : parcours profonds (5+
  sauts, plus courts chemins) sur des millions d'objets en interactif.
- **Hub de kits** (`hub.md`, ajout 2026-10-10) — **gelé jusqu'à la
  0.2.0** : partage communautaire de flows, dashboards, synoptiques,
  fonctions et variantes de cartes en kits (Git + CI + index statique,
  zéro backend marketplace) ; références locales converties en slots et
  requirements, classes de sensibilité déclarées sur les schémas
  `pnex-core` pendant la refacto ontologie ; install depuis l'UI,
  enregistré non déployé. Rien côté code pendant le gel. Décision #23.
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
| 9 | Firmware custom : ~~nœud flow des commandes (D88)~~ tranché D146 (Device (write)), 1er lot du catalogue (D92), technique de sandbox V1 (D91, à aligner avec P1.6) | Au lancement de P2.10 |
| 10 | Parité n8n : périmètre V1 (logique de flux, déclencheurs, credentials, quelles intégrations d'abord) et descripteur généré vs inspecteurs dédiés | Au PRD de P2.11 |
| 11 | API publique : transport des abonnements live (WebSocket vs SSE vs REST), modèle de jetons (portée device/flow, lecture/écriture, rotation), lien avec les déclencheurs webhook de P2.11 | Au passage de P3 à P2 |
| 12 | ~~Base de dev antérieure à D120~~ — tranché 2026-10-03 : tout détruit, `pnex` recréée (O18) | ✅ |
| 13 | ~~Jetons device dans l'URL des WebSockets~~ — tranché 2026-10-09 : en-tête `Authorization` partout (lot L2 de D153, `security-tiers.md` §6 bis), jeton en URL ignoré | ✅ |
| 14 | ~~Version firmware par rebuild~~ — tranché 2026-10-03 : 1 build = 1 enregistrement = 1 version, OTA en lot manuelle (O22) | ✅ |
| 15 | Profils de sécurité : ~~date de lancement de V1 (correctifs protocole, SEC-17 à SEC-19)~~ ; socle de V4 (ESP-IDF C++ ou firmware Rust) ; statut CRA de PneX (avis juridique) | V1 : ✅ livrée le 2026-10-09 (D153–D158) ; V4 : après 1 à 2 ans de communauté |
| 16 | Flux média : runtime ASR (sherpa-onnx, whisper.cpp ou les deux), ~~tags Loco ou queue dédiée~~ (tranché à la relecture du 2026-10-09 : tags, ASR configurable), superviseur de capture in-process (proposé), agrégation « par plage » (primitive D182), ~~plafond des extraits, amendement de D3~~ (sans objet : pas de publication), ordre vis-à-vis de l'ontologie (`media-ingest.md` §14) | Lot 0 (POC ASR) ; ordre à la validation |
| 17 | Ontologie 0.2.0 : ~~**actions D183 remontées dans la 0.2.0 ?**~~ (oui : livrées le 2026-10-13) (au moins une action simple tracée : c'est ce qui rend la plateforme « opérationnelle », et le pack Maintenance et les procédures de `pages.md` en dépendent) ; ~~identifiant d'objet global (UUID vs `ResourceRef`)~~ (tranché 2026-10-10 : UUID par table d'identité universelle), moment du bitemporel, format du schéma de propriétés (maison vs JSON Schema), liaison device → objet (lien générique vs table dédiée), packs forkables ou surcouche, langage de requête textuel, spécification publique du noyau ; ordre P2.13 / P2.14 / P2.1 vu le gel des nouveaux piliers (`ontology.md` §9) | À la validation du PRD |
| 18 | ~~Media & Vision Studio : pose humaine ou poste de travail, tract vs `ort`, entraînement CPU, D174 en usage interne, détection → `device_write`, version cible~~ — **tranché 2026-10-13** (`media-vision-studio.md` §10) : poste de travail, tract seul, entraînement en Rust sans Python, détection → `device_write` régie par D128, pas d'attente de tag | ✅ |
| 19 | ~~Garder SQLite à côté de PostgreSQL~~ — tranché 2026-10-10 : **PostgreSQL obligatoire, SQLite abandonné**. La parité coûte à chaque migration et chaque requête (`ilike`, FK par reconstruction de table, pas de PostGIS) et pèse de plus en plus lourd à mesure que le produit grandit ; l'argument Raspberry Pi ne tient pas, Postgres reste léger sur un Pi. Les nouveaux développements ne gèrent plus SQLite | ✅ |
| 20 | Couches géographiques : ~~gel 0.2.0~~ (tranché 2026-10-10 : phase F avant, A–E après), ~~rétention des seeds~~ (génération courante), raster, OGC API – Features, plafonds par tier, lien ontologie (`geo-layers.md` §10) ; image Postgres étendue PostGIS + pgvector commune avec #22 | À la validation du PRD |
| 21 | Pages : Loro vs yrs (`pages.md` §11). Tranché 2026-10-10 (`pages.md` §13) : L1–L2 en exception au gel, tableau local admis (formule = lecture, `ontology.md` §3 amendé), pas de pages privées (suit D188), mesure publiée en série `*_manual` seulement, signature = trace d'identité en V1, nom « Pages » | Loro/yrs au spike L0 |
| 22 | Documents & recherche IA : BM25 ParadeDB vs `ts_rank_cd`, rerank, poids de DataFusion sur Pi (`doc-search.md` §10). Tranché 2026-10-10 (§13) : P1 seul en exception au gel, embeddings en tract, index partagé avec les pages | À la validation du PRD ; BM25 au jeu de test |
| 23 | Hub de kits : kits hardware-first, dépendances entre kits, hébergement de l'index (`hub.md` §19). Tranché 2026-10-10 (§21) : kit = pack D190 (un seul format), identité = compte GitHub seul | Pendant la 0.2.0 (aligner les schémas), implémentation après |
| 24 | Dossiers : ~~version cible (C0–C1 en exception au gel ?)~~ (sans objet : C0–C7 sur `main` le 2026-10-12), nom « Dossiers / Cases », ACL par dossier (V2, avec D188), temps flou dans le noyau, pack de base « Acteurs », co-présences GPS, horodatage qualifié des gels, seuil d'écriture automatique des apparitions, identification vocale par enrôlement consenti (K19, avis juridique requis) (`cases.md` §11) | À la validation du PRD |
| 25 | ~~Stockage `fs` local à côté de S3~~ — tranché 2026-10-12 : **fin du stockage `fs`, toujours RustFS ou un S3 compatible** (médias, firmware, modèles, frames de stitch). Deux backends = trop de code à maintenir ; l'argument Raspberry Pi ne tient pas, RustFS ne consomme quasiment rien pour le volume de données traité. Les nouveaux développements ne gèrent plus `fs` | ✅ |
| 26 | Calendrier GMAO : vue générique des plages (noyau) ou vue du pack ; ~~occurrence = plage D182 ou table dédiée~~ (plage D182, premier lot) ; mode fixe/flottant ; horizon de matérialisation (90 jours au premier lot) ; calendriers ouvrés en v1 ; date prévisionnelle d'un compteur ; hors ligne terrain (`ontology.md` annexe B11) | À la relecture de `modele-cible.md` (B12) |

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
  nœuds augurs → linfa + registre). Stockage des modèles = RustFS
  ou Postgres.
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
- **2026-10-03 (suites e2e)** — O18–O24 traités (base recréée, logs sans
  jetons edge/compose/Helm, handshake WS 5 s, notification sur front
  montant, 1 build = 1 version + OTA en lot manuelle, mineurs UI/Docker).
  Ouvert : dette a11y (O21), suite e2e en CI (O25). Licence O20 tranchée le
  jour même : ArduinoWebsockets (GPL-3) remplacée par le client WS maison
  `pnex_ws` (Apache-2.0).
- **2026-10-04 (surfaces pilotables)** — D123–D129 livrés (L1–L7,
  `surfaces-controls.md`) : dashboards PC/mobile, contrôles d'org, nœud
  `control-source`, ajout guidé depuis un device avec « Créer le flow »,
  items d'annotation `control` / `reading`. Reste : e2e matériel
  interrupteur → LED réelle (banc P0.1).
- **2026-10-04** — Ajout P2.12 : dashboards « Maison » (D134–D141, plan
  validé) ; mode sombre global (REP-1) et code PIN serrure/alarme (REP-2)
  reportés et consignés.
- **2026-10-08** — Ajout en P3 : profils de sécurité du maker au
  militaire (`security-tiers.md`, D148–D152), décision #15. Exigences
  tracées, implémentation par vagues ; eFuses gelés jusqu'à 1 à 2 ans de
  validation communautaire du firmware. Registre : SEC-17 à SEC-20.
- **2026-10-09 (média + ontologie)** — Deux PRD proposés, rien
  d'implémenté : P2.13 ingestion de flux média (`media-ingest.md`,
  D159–D175, décision #16) et P2.14 noyau ontologique de la 0.2.0
  (`ontology.md`, D176–D191, décision #17). Numérotation décalée de +11 à
  l'intégration (D148–D158 déjà pris par `security-tiers.md`).
- **2026-10-10 (Media & Vision Studio)** — PRD proposé, rien
  d'implémenté : P2.15 (`media-vision-studio.md`, décision #18), relu
  contre la doc (stockage RustFS, tract, kinds existants, LLM
  d'org D119, conflits D173/D174 et gel 0.2.0 signalés).
- **2026-10-10 (PostgreSQL seul)** — Décision #19 : SQLite abandonné,
  PostgreSQL obligatoire ; plus aucune parité à écrire dans les nouveaux
  développements ; retrait du code SQLite existant le jour même (branche
  `refactor/postgres-only`). Axe G :
  références et architecture pressentie du geofencing (Tile38, Traccar,
  ThingsBoard, Home Assistant, PostGIS accepté en extension malgré la
  GPL-2, crates `geo`/`rstar`/`h3o`/`geozero`).
- **2026-10-10 (quatre PRD)** — Intégrés et relus contre la doc, rien
  d'implémenté : P2.16 couches géographiques + fournisseurs géo
  (`geo-layers.md`, décision #20), P2.17 pages collaboratives
  (`pages.md`, #21), P2.18 documents & recherche IA (`doc-search.md`,
  #22 ; stockage = MediaStore, pas de table `document`, pas de LLM
  plateforme), hub de kits gelé jusqu'à la 0.2.0 en P3 (`hub.md`, #23 ;
  pas de WASM ni de Python dans la stack, UI = interface utilisateur).
  Numéros D attribués à la validation, après la réserve D192+ de
  P2.15.
- **2026-10-10 (arbitrages des quatre PRD)** — Géo : phase F avant la
  0.2.0, A–E après, seed = génération courante. Pages : L1–L2 en
  exception au gel, tableau local admis (`ontology.md` §3 amendé), pas
  de pages privées, mesures manuelles en série `*_manual`, signature =
  trace d'identité, nom « Pages ». Recherche IA : P1 seul en exception,
  embeddings en tract. Hub : kit = pack D190, identité GitHub seule.
  Restent ouverts : choix techniques à trancher au spike ou au jeu de
  test (Loro/yrs, BM25, rerank, DataFusion, raster, OGC).
- **2026-10-10 (annexe ontologie & stockage graphe)** — Intégrée dans
  `ontology.md` : objets temporels comme les liens, identité universelle
  (D177 amendé, Q1 tranchée), interface graphe unique (CTE),
  API bornée à 4 sauts et illimité en interne, PostgreSQL 19 visé pour la
  0.2.0, vue graphe au L4, Apache AGE en réserve (P3), alternatives
  écartées (Neo4j, SurrealDB, XTDB), positionnement 0.2.0.
- **2026-10-10 (PostgreSQL 19 bêta)** — Stack de dev et CI passées sur
  `postgres:19beta4-alpine` (base de référence appliquée sans
  changement). Constat : **SQL/PGQ n'est pas dans PG 19** ; l'interface
  graphe de l'ontologie démarre en jointures + CTE.
- **2026-10-10 (retour à PostgreSQL 18)** — 19beta4 abandonné le jour
  même : sans SQL/PGQ, PG 19 n'apporte rien au graphe et retarderait
  PostGIS et pgvector. Graphe de l'ontologie = CTE + `petgraph`
  (annexe A3, avec la comparaison Palantir Foundry / ArgonOS) ;
  pgRouting et AGE en branchements optionnels. Question « actions D183
  dans la 0.2.0 ? » ouverte dans la décision #17.
- **2026-10-11 (dossiers)** — Ajout du PRD `cases.md` (P2.19, décision
  #24) : espace de travail borné commun à la RCA, à l'enquête sur
  données, au vidéaste et au journaliste ; chronologie unifiée comme
  primitive du noyau, acteurs humains et affirmations sourcées en pack,
  confinement par org dédiée en V1.
- **2026-10-11 (positionnement)** — PneX passe en mode **plateforme de
  données** sur socle IoT (section « Positionnement ») : la donnée reliée
  (ontologie, séries, médias, documents, pages, géo, dossiers) devient le
  cœur ; contrôle/commande, M2M, firmware/OTA, identité device et
  sécurité gardent la même ambition et la même roadmap.
- **2026-10-11 (outils externes)** — Section « Outils externes évalués »
  (après P2.19) : anydoc et fframes au cœur, références de design,
  sidecars (Stirling-PDF, navigateurs headless), Typst candidat,
  tutoriels générés dans `pnex-website`. Reticulum écarté (M2M =
  zenoh-pico + OpenThread inchangé) ; pas de plafond < 1 Go.
  Rien d'implémenté.
- **2026-10-12 (analyse du discours)** — PRD `discourse-analysis.md`
  (DA1–DA10) : formules récurrentes détectées mot pour mot (empreintes de
  fragments), mesures de pluralisme sourcées, indices de coordination
  documentés sans conclusion automatique ; emplacement du composant
  ouvert (proposé : outil de l'assistant depuis une note).
- **2026-10-12 (dossiers, refonte)** — PRD `cases-workspace.md` v0.2 et
  benchmark de 6 projets (OpenAleph, OpenCTI, Timesketch, DFIR-IRIS,
  kepler.gl, MovingPandas) : brut et retenu séparés, événement au noyau,
  traces O2, graphe natif ; amende K1, K2, K6 de `cases.md`.
- **2026-10-12 (S3 seul)** — Décision #25 : fin du stockage `fs`,
  toujours RustFS ou un S3 compatible ; même raisonnement que #19 (coût
  de maintenance de deux backends, RustFS négligeable sur un Pi). Retrait
  du code `fs` existant à planifier.
- **2026-10-12 (Media & Vision Studio, M1)** — P2.15 lancé (gel 0.2.0
  levé pour ce pilier par Shan, comme P2.13) : D192 suivi d'objets
  (ByteTrack Rust, glouton, vitesse constante) et D193 zones d'image
  (polygones normalisés, entrée/sortie/occupation, présence minimale),
  éditeur de zones sur la vue live. Q10 tranchée dans ce sens.
- **2026-10-13 (Media & Vision Studio, M3)** — D194 (D174 rouverte pour
  l'usage interne, sorties marquées IA), D195 `vision_caption` (image lue
  par le backend, modèle de l'org), D196 `media_fuse` (fenêtre → phrases
  sourcées, rejet des phrases sans source, plafond 2000 appels/jour/org).
- **2026-10-13 (Media & Vision Studio, M3bis)** — D197 `text_decide` :
  Laya multilingue exécuté par tract dans le backend (Q12 et Q15
  tranchées), bundle construit par `task laya:bundle`.
- **2026-10-13 (Media & Vision Studio, M2a)** — Vision Lab côté données :
  projets à classes positionnelles, annotation à la souris d'images de la
  médiathèque, split par source, versions figées, export COCO (D198–D200).
- **2026-10-13 (Media & Vision Studio, questions)** — Q1–Q15 toutes
  tranchées (`media-vision-studio.md` §10) : « poste » = poste de
  travail, tract seul runtime, détection → `device_write` régie par D128,
  pas d'attente de tag (0.1.0 épinglée, 0.2.0 annoncée sans promettre de
  fonctionnalités), INT8 obligatoire sur Pi pour `text_decide`, seuil de
  production ≥ 80 % / ECE ≤ 0,10.
- **2026-10-13 (Media & Vision Studio, M2b → M5)** — Règle « aucun
  Python » : entraînement = tête YOLOX réapprise en Rust (D204). Vidéos
  dans le Lab et interpolation (D203), file « à revoir » et guidage de
  diversité (D205), fichier de la médiathèque comme source de flux
  (D206). Rapports (D207–D210) : pas de studio, une page écrite par des
  nœuds génériques, PDF krilla, pièce jointe e-mail, approbation humaine
  par défaut. Développé sur `feat/media-vision-studio`, mergé dans `main`
  le 2026-10-13.
- **2026-10-13 (calendrier GMAO)** — Nouveau P2.20 : phase d'étude
  clean-room de projets GMAO, calendrier et récurrence open source
  (`ontology.md` annexe B), décision #26.
- **2026-10-13 (maintenance)** — D183 actions remontées dans la 0.2.0
  (#17) et premier lot du calendrier de maintenance (annexe B13) sur
  `feat/gmao-plans` ; SEC-35 (plages d'objet sans garde du type) corrigé
  au passage.
- **2026-10-13 (revue)** — Resynchronisation : tableau d'état et titres
  P2.13 → P2.19 alignés sur le code de `main` (géo phase F, pages L1–L2,
  recherche P1, dossiers et espace de travail, empreinte vocale, lots
  média 0–6) ; nouvelle section « Branches mergées le 2026-10-13 » ; 0.1.0 épinglée.
- **2026-10-14 (P0.3)** — Cluster déroulé sur dev.pnex.io (3 pods, device
  réel) : bascules, reprise d'org et bus device D107 validés ; rolling update
  à corriger (O67, `preStop` + pods flow-worker dédiés dans `pnex-deploy`).
  Retours d'usage de l'éditeur de flows consignés (O68–O70) : la partie
  flows doit devenir intuitive pour un non-développeur, même avec beaucoup
  de nœuds (groupes, zoom, valeurs déclarées sur le nœud de sortie) —
  chantier à cadrer après les urgences en cours.
- **2026-10-15 (flows)** — Groupes de nœuds et valeurs déclarées au canevas
  sortis d'O70 vers P2.21 (gros chantier, plus tard).
- **2026-10-15 (observations)** — Non commité : valeur déclarée par sortie
  du `Device (écriture)` (O70), résilience firmware ESP32 (watchdog de
  boucle, redémarrage après 10 min sans session, TLS borné, O71), logs du
  runtime assainis (O72), défauts visuels de l'éditeur (O73), recherche
  globale dans le contenu des documents (O54), `word_timestamps` appliqué
  (O56), suite des listes CRUD (O64). Docs d'architecture alignées
  (`flow-engine.md` §10, `edge-model.md` §3, `design-system.md` règle 8).
- **2026-10-17 (positionnement)** — Ajout de P2.23 : observabilité IT
  (OTLP logs/traces/métriques, Prometheus scrape + `remote_write`, syslog,
  Loki, InfluxDB, StatsD). PneX = toute donnée, pas seulement le physique ;
  rien d'implémenté. Site : slogan « plateforme de données ouverte et
  souveraine », carte « Secteur public & enquêtes ».
- **2026-10-17 (sécurité logicielle)** — Ajout de P2.24 : SBOM
  (CycloneDX, SPDX), VEX, SARIF, scans et qualité du code ingérés et reliés
  à l'ontologie, SBOM par build firmware, état des pipelines CI/CD, builds,
  couverture et rapports de tests ; rien d'implémenté.
