# PRD — PNEX Media & Vision Studio

> **Statut :** v0.3 — **validé et lancé le 2026-10-12** (Shan : « attaque
> la partie studio », M1 en premier). v0.1 de Shan, relue contre la doc le
> 2026-10-10 (§11). Décisions actées au §12 ; questions tranchées au §10.
> **Auteur :** Shan
> **Périmètre :** pilier « Ingestion média + Vision » de PNEX (audio,
> vidéo, caméras IP, reconnaissance d'objets custom, description vidéo,
> Vision Lab d'entraînement).
> **Version cible :** pas d'attente de tag (Q10, 2026-10-13) : la 0.1.0
> est épinglée, la 0.2.0 annoncera ce qu'elle livre sans promettre les
> fonctionnalités à l'avance (beta). Entrée roadmap : P2.15, décision #18.
> **Numérotation :** D192 et suivants, attribués au fil des lots (§12).
> **Renvois :** `camera-video.md` (D73–D86 bus caméra, D81 registre de
> modèles, D82 `pnex-vision`/tract, D83 `vision_detect`, D84 événements
> O2, D100–D104 fiabilité, D105 calques de détection), `media-ingest.md`
> (D159–D175 : flux, capture, ASR, `media_source`, D173/D174 rapports et
> synthèses reportés, D175 caméras IP sur le bus), `media-asr-bench.md`
> (lot 0 ASR), `ml-vision.md` (pilier ML, stack tout-Rust),
> `worker-fabric.md` (P2.7, workers GPU), `media.md` (D21 bibliothèque
> média, versioning), `ontology.md` (D176–D191, provenance D184, rapports
> D187), `secrets.md` (D119 LLM d'org), `surfaces-controls.md` (D128
> frontière d'actionnement), `security.md` (R1–R20), `ai-assistant.md` §9.

---

## 0. Ce qui existe déjà (à ne pas refaire)

Ce PRD **étend** deux chantiers existants, il ne les remplace pas :

| Besoin du PRD | Déjà décidé / livré | Ce que ce PRD ajoute |
|---|---|---|
| Sources média (RTSP, HLS, radio) | `media_streams` (D159), capture ffmpeg confinée derrière fetcher filtré (D160 : icecast, HLS, `http_file` ; piste audio extraite des flux vidéo) — **livré** (lot 1, porteur `server`) ; RTSP/DASH/DVB, porteur `worker` (1h) et caméras IP sur le bus (D175, lot 6) à faire | Source « fichier uploadé » rejouée depuis la bibliothèque média (D21) |
| Transcription horodatée | `pnex-asr serve` (process confiné, sherpa-onnx par défaut, whisper.cpp en option), worker `transcribe_segment` (D166), O2 `tx_<slug>` + recherche, registre de modèles audio (D167), rétention D161, UI `/streams` — **livré** ; nœud `media_source` (D163, lot 2) à faire | Rien côté ASR |
| Détection d'objets | `ml_models` + `pnex-vision` (tract, YOLOX Apache-2.0), nœud `vision_detect`, test image/live, validation à l'enregistrement (D81–D83, D100–D104) — **livré** | Tracking, zones, modèles custom |
| Événements interrogeables | Nœud `event_log` → O2 `ev_*` (D84), calques `camera_detections` (D105), notifications (D85) | Événements dérivés (zone, présence, comptage) |
| Registre de modèles | Bibliothèque média `kind=model` + versions (D21, D81), check par porteur (D167) | Lien modèle → dataset figé + run d'entraînement |
| Rapports, synthèses IA | **Reportés** (D173, D174) ; D3 : rapports = OpenObserve | Rouvre la question (M5), voir §4.8 |

## 1. Contexte et vision

PNEX glisse d'une plateforme IoT vers une **plateforme data unifiée**
(vision long terme : type Palantir / ArgonOS ; socle = ontologie 0.2.0).
Après le firmware, les devices, l'ETL, le dashboarding, le Studio
Panorama 360, les synoptiques SCADA, la caméra (D73–D105) et l'ingestion
de flux audio (D159–D175), il reste à couvrir **la compréhension des
médias** : ce qui est vu, détecté, et la synthèse de l'ensemble.

L'objectif est de transformer n'importe quel flux média en **événements
horodatés, structurés et interrogeables**, au même rang que la
télémétrie IoT. Les sources de ces événements sont :

- ce qui est dit (transcription, **déjà couvert** par `media-ingest.md`) ;
- ce qui est vu (description visuelle) ;
- ce qui est détecté (objets custom, éventuellement postures).

Ces événements alimentent l'alerting, les dashboards, les synoptiques et,
à terme, un **studio de rapports intégré** (aujourd'hui reporté, §4.8).

L'utilisateur doit aussi pouvoir **créer ses propres modèles de
reconnaissance d'objets** sans compétence ML. C'est le rôle du **Vision
Lab** (le « Model Lab » de l'axe C de la roadmap, volet vision) : il joue
une vidéo ou ouvre des images, entoure l'objet sous plusieurs angles, et
PNEX génère, évalue et déploie le modèle.

### Principes directeurs (réflexions de Shan)

| # | Principe | Implication |
|---|---|---|
| P1 | **Une seule surface, zéro basculement d'outil** | Annotation, entraînement, rapports et visualisation se font dans l'UI PNEX (Dioxus). Pas de CVAT, Label Studio ou Jupyter exposés à l'utilisateur. |
| P2 | **Identité visuelle unifiée** | Tout moteur tiers reste headless derrière l'UI PNEX. |
| P3 | **Écosystème cohérent, pas « 500 dépendances »** | Briques minimales, réutilisation de la stack existante (Loco, Postgres, OpenObserve, Valkey, moteur de flows, queue Loco, `pnex-vision`, `pnex-asr`, bibliothèque média). |
| P4 | **Licences permissives préservées** | Aucune dépendance runtime copyleft (AGPL et autres) ; toute brique MIT/BSD/Apache, LICENSE réel lu. **Détection = YOLOX exclusivement** (Apache-2.0). Ultralytics exclu. |
| P5 | **Tourne sur Raspberry Pi** pour le cœur | Capture, ASR léger et détection nano/tiny possibles à l'edge. VLM, LLM et entraînement sont déportés sur un worker GPU (fabric P2.7) ou une API. |
| P6 | **La plateforme n'actionne jamais d'elle-même** | Le serveur collecte, transforme, alerte et notifie. Les nœuds vision n'émettent aucune commande. Seul un flow **déployé par l'humain** avec `device_write` touche un actionneur (D128) — la vision ne change pas cette frontière. |
| P7 | **Ontologie optionnelle** | Une caméra ou une source média fonctionne seule. L'ontologie (objets, placements, temporalité) enrichit sans être obligatoire (types système dans leurs tables, D177). |
| P8 | **Open source, modèles open weights** | ASR, détection et VLM en open weights, licence de chaque poids vérifiée. Les appels distants passent par le **LLM / endpoint de l'org** (D119), jamais par un fournisseur plateforme. |

---

## 2. Problèmes adressés

1. Les flux vidéo (enregistrements, caméras IP) restent des **données
   peu exploitées** : la détection COCO existe (D83), mais sans suivi
   d'objet, sans zones, sans description.
2. Les modèles de détection génériques (classes COCO) ne reconnaissent
   pas les **objets métier** : pièces, équipements, EPI, outils, produits.
3. Créer un modèle custom demande aujourd'hui une chaîne d'outils ML
   hétérogène (annotation, scripts, GPU, export), **inaccessible aux
   utilisateurs non techniques**. Le registre actuel n'accepte que des
   ONNX produits ailleurs.
4. Produire un **rapport** à partir d'heures de média est manuel.

---

## 3. Personas

| Persona | Besoin |
|---|---|
| **Maker / DIY** | Brancher une caméra IP ou un micro, avoir une transcription, détecter « son » objet. Démarrage en quelques minutes. |
| **Opérateur industriel / superviseur** | Surveiller un poste ou une zone, recevoir des alertes (objet absent, EPI manquant), consulter un résumé horodaté. |
| **Analyste** | Fouiller des heures de média, corréler aux données capteurs, générer des rapports. |
| **Intégrateur / admin** | Déployer sur Pi ou cloud, brancher un worker GPU, gérer modèles et versions. |

---

## 4. Périmètre fonctionnel

### 4.1 Sources média

- **Réseau** : `media_streams` (D159) couvre déjà Icecast/HLS/DASH,
  RTSP/ONVIF et la piste vidéo des caméras IP (D175 : frames JPEG publiées
  au format du bus caméra, `fps` configurable). Capture ffmpeg **sans
  réseau** derrière le fetcher filtré R8 (D160). Rien à redéfinir ici.
- **Fichier uploadé (ajout)** : une vidéo ou un audio déposé dans la
  bibliothèque média (D21) peut être **rejoué** comme source : mêmes
  sorties (frames sur le bus, segments audio vers l'ASR), horloge = temps
  du fichier. Jamais d'URL `file:` ni de chemin local (D160).
- Une source n'a pas besoin de l'ontologie (P7) ; quand L1 existera,
  `media_stream` est un type système (D177).
- Paramètres : fps d'analyse (D175), fenêtre d'agrégation (nœuds), et
  rétention **séparée** médias bruts / événements (D161 pour l'audio,
  D79 pour les segments vidéo, rétention O2 pour les événements, D72).

### 4.1bis Analyse d'un fichier de la médiathèque (D211, **proposé, à relire**)

> Branche audio du **pipeline d'import** (`media-import-pipeline.md`,
> D212–D217) : l'analyse y est lancée par `media_analyses` (D213), qui
> tranche Q-A (proposé, jamais automatique) et Q-D (flux interne caché,
> lié à l'analyse).

**Besoin** (Shan, 2026-10-14) : après l'upload d'un audio ou d'une
vidéo, un bouton **« Analyser »** qui extrait la piste audio, détecte les
voix, transcrit et range le résultat dans O2, pour enchaîner des analyses
automatiques (flows, rapports M5).

**Pourquoi D206 ne suffit pas** : D206 rejoue le fichier comme un direct.

| | D206 (aujourd'hui) | Besoin « fichier » |
|---|---|---|
| Vitesse | `-re` : 1 h de fichier = 1 h de traitement | aussi vite que l'ASR le permet |
| Horodatage | heure de relecture (contredit le « horloge = temps du fichier » du §4.1) | position dans le fichier, stable d'une analyse à l'autre |
| File ASR | segments `live`, abandonnés au-delà de `PNEX_ASR_MAX_LAG_SECS` (600 s) | jamais abandonnés, toujours **derrière** le direct |
| Parcours UI | créer un flux, l'activer, attendre | un bouton sur l'asset, une progression, un résultat |
| Fin | flux désactivé, rien d'autre | événement « analyse terminée » exploitable par un flow |

**Principe : pas de second pipeline.** Une analyse **est** un flux
`media_asset` (D206) avec une **cadence** `pace = batch` au lieu de
`realtime`. Tout le reste est réutilisé tel quel : décodeur confiné D160
(+ `moov` en tête D203), segments `media_segments`, worker
`transcribe_segment`, VAD + diarisation (lot 5), O2 `tx_<slug>`,
`media_source`, recherche, rétention D161 (l'audio des segments est
purgé ; l'original reste dans la médiathèque).

Ce qui change, et seulement pour `pace = batch` :

1. **Décodage sans `-re`** (comme le bouton Test aujourd'hui), audio
   seulement : `tracks: video` refusé en batch (des images à 50× la
   vitesse inonderaient le bus caméra et `vision_detect` ; la vision sur
   fichier reste le Lab, D203).
2. **Contre-pression** : le décodeur s'arrête tant que le flux a plus de
   N segments en attente (`queued`, N = 4) : pas de file de 200 segments
   ni de stockage qui gonfle.
3. **Horloge fichier** : `started_at = base + offset`, `clock_source =
   file`, avec `base` = date de création de la **version média**
   analysée (déterministe : ré-analyser la même version retombe sur les
   mêmes instants). Chaque document O2 porte en plus `offset_ms` (position
   dans le fichier) que l'UI affiche (« 12:34 »).
4. **File ASR** : enqueue `live = false` (priorité sous le direct sur une
   file `asr` dédiée, comportement déjà codé dans `segments::priority`),
   **exempté** du contrôle `max_lag` (le retard est normal en batch).
5. **Fin** : quand le dernier segment est transcrit (pas quand le
   décodeur s'arrête), le flux passe `finished` et publie `{kind:
   "finished", segments, speech_ms}` sur son canal Valkey ; `media_source`
   l'émet sur un **second port** `finished` (déclencheur de la chaîne
   `llm_text` → `page_write` → `page_pdf`, D207).

**UI** (médiathèque, assets `audio` / `video`) :

- bouton **« Analyser »** (éditeurs ; lecteur → 403, R2) → choix du
  profil ASR (diarisation incluse si le profil l'a) → crée ou réutilise
  **le** flux batch de cet asset (slug `file-<asset court>`, un par asset)
  et le démarre ;
- état sur l'asset : en attente / x % (offset du dernier segment ÷ durée)
  / terminé / échec (code traduit) ; **« Voir la transcription »** = la
  vue segments existante, locuteurs S1, S2… ;
- la liste des flux masque les flux batch par défaut (filtre « fichiers
  analysés »), ils restent sélectionnables dans `media_source`.

**Garde-fous** :

- org de l'asset re-vérifiée au démarrage et à chaque lecture (R1, déjà
  fait par D206) ; écriture = `can_write` + test lecteur → 403 ;
- **une analyse batch à la fois par org** (sémaphore, comme D204 / SEC-37),
  une seconde → 409 `media-analysis-busy` ; durée ≤ 4 h, taille ≤ 512 Mo
  (borne D206) → `media-analysis-too-long` ;
- l'équité ASR par org (`PNEX_ASR_MAX_PER_ORG`) s'applique aussi au batch ;
- texte transcrit = **donnée non fiable** (comme en direct) : jamais
  passé comme instruction en aval.

**Hors périmètre de D211** : identification des locuteurs par Voice ID
(voir questions), vision sur fichier (Lab D203), déclenchement
automatique à l'upload (voir questions), traduction.

**Livrables du même commit** : migration (`media_streams.pace`,
`offset_ms` dans le document O2), codes d'erreur (`err_codes::ALL` + deux
`.ftl`), clés UI, `NodeDoc` de `media_source` (port `finished`), fiches
KB `media` et `streams`, tests : cadence (aucun `-re`), contre-pression,
horloge déterministe, exemption `max_lag`, 409 seconde analyse, lecteur
403, asset d'une autre org → 404, port `finished` émis une fois.

**Questions ouvertes (à trancher avant code)** :

- **Q-A — Déclenchement** : bouton seul (proposé) ou aussi « analyser
  automatiquement à l'upload » (réglage d'org) ? L'automatique coûte de
  l'ASR à chaque dépôt, y compris pour des fichiers sans intérêt.
- **Q-B — Ré-analyse** : remplacer (proposé : les documents O2 de la
  même version tombent aux mêmes instants ; on ajoute un label `run` et
  la vue ne montre que le dernier) ou garder chaque analyse comme
  version comparable (utile pour comparer deux modèles ASR) ?
- **Q-C — Voice ID** : proposer, à la fin d'une analyse, la mise en
  correspondance des locuteurs S1/S2 avec les profils vocaux de l'org
  (K19, **proposition** jamais écrite seule, consentement déjà exigé) ?
  §4.2 dit « jamais d'identification vocale » dans l'ingestion : ce
  serait une exception explicite, avec un avis juridique toujours manquant.
- **Q-D — Un flux par asset ou une table `media_analyses`** : le flux
  par asset réutilise tout, mais pollue la liste des flux (d'où le
  filtre). Une table dédiée serait plus propre en UI, au prix d'un
  second chemin vers les segments et O2. Proposé : flux par asset.

### 4.2 Branche audio → transcription horodatée

**Entièrement couverte par `media-ingest.md`** (D162–D167) : VAD +
ASR via `pnex-asr` (sherpa-onnx par défaut, whisper.cpp en option de
build ; pas de faster-whisper, qui est une stack Python), segments
`{t0, t1, texte, langue, confiance}` dans O2 `tx_<slug>`, diarisation au
lot 5 (labels locaux, jamais d'identification vocale). Modèle léger sur
Pi, large sur worker : profils D164 + check par porteur D167.

### 4.3 Branche détection d'objets (YOLOX)

- **YOLOX exclusivement** (P4), via le registre existant (`ml_models`,
  famille `yolox`) :
  - pré-entraînés COCO (déjà supportés) ;
  - custom issus du Vision Lab (§ 4.6).
- Inférence : **`pnex-vision` (tract)**, déjà en production (D82),
  dans le nœud existant `vision_detect`. `ort` (bindings ONNX Runtime)
  n'est ajouté que si tract plafonne au bench Pi ; Burn reste le
  challenger documenté (`ml-vision.md` étape 1).
- **Tracking ByteTrack** (algorithme MIT) : réimplémenté en Rust (Kalman
  + association IoU, sans dépendance Python) dans un nouveau nœud
  `vision_track` ; id persistant par objet.
- Agrégation des détections en **événements** :
  ```json
  {"t0": 12.0, "t1": 100.4, "class": "chariot_elevateur", "track_id": 3,
   "conf_avg": 0.91, "zone": "quai_2", "event": "present"}
  ```
- **Zones** : polygones dessinés **sur l'image de la caméra**
  (coordonnées image normalisées 0–1, par source). C'est un concept
  nouveau : le geofencing PNEX n'existe pas encore (simple ouverture
  après G1/G2, `roadmap.md`), et ses zones seraient en WGS84.
- Événements dérivés : apparition, disparition, entrée ou sortie de
  zone, durée de présence, comptage. Écrits via `event_log` (O2 `ev_*`,
  D84) ; les cadres bruts continuent d'aller en calques
  `camera_detections` (D105).

### 4.4 Branche vision descriptive (VLM)

- VLM open weights qui produit une légende factuelle d'une keyframe ou
  d'un mini-clip.
- **Crop puis VLM** : description fine d'un objet détecté (état,
  couleur, texte lisible).
- Exécution sur worker GPU (endpoint OpenAI-compatible, ex. vLLM) ou
  API distante, **configurée au niveau de l'org** (D119 : pas de LLM
  plateforme), appel sortant filtré (R8). Jamais exigé sur le Pi.
- Le VLM est un fournisseur de l'org comme le LLM ; l'envoi d'images à
  une API distante est **explicite par source** (§6 confidentialité).

### 4.5 Fusion → description vidéo

- Par fenêtre temporelle (10 à 30 s, paramétrable), le moteur assemble
  trois entrées :
  1. la transcription (`tx_<slug>`) ;
  2. les événements de détection ;
  3. les légendes VLM.
- Le **LLM de l'org** (D119, aucun modèle par défaut imposé par la
  plateforme) produit une **description unifiée horodatée**, puis un
  **résumé global**.
- Hiérarchie de confiance imposée dans le prompt :
  - **détections** = faits ;
  - **ASR** = fiable pour la parole ;
  - **VLM** = indicatif.
  - Le LLM n'invente aucun objet absent des détections ou des légendes.
- **Texte non fiable** (règle de D168) : transcriptions et légendes sont
  passées comme données délimitées, jamais comme instructions ; aucun
  outil exposé ; plafond d'appels par org et par jour.
- Traçabilité : chaque phrase générée référence ses sources (ids de
  segments ASR, d'événements, de légendes), au format de provenance
  D184 ; une phrase sans source est rejetée, pas affichée.
- Toute sortie est marquée « générée par IA » (règles de D174).
- **Portée de D174** : la description par fenêtre est une synthèse IA ;
  ce PRD rouvre donc D174 **pour l'usage interne** (consultation dans
  PNEX), sans publication. Décision à prendre à la validation.

### 4.5bis Étage de décision rapide (`text_decide`)

Entre les sorties texte du pipeline (segments ASR, événements de zone,
légendes, description `media_fuse`) et l'alerting, un étage de
**décision typée** classe chaque fenêtre sans générer de texte.

- **Modèle candidat : Laya** (Convai Innovations, poids **Apache-2.0**,
  conforme P8). Encodeur bidirectionnel + tête de décision, **non
  génératif** : il reçoit un état (texte ou JSON) et des questions
  typées, et renvoie une distribution de probabilités par question, en
  une seule passe pour toutes les questions.
- **Primitives exposées** :
  - `choice` : une option parmi N, probabilité par option ;
  - `score` : niveau attendu sur une échelle ordonnée + distribution ;
  - `noul` : P(vrai) d'une affirmation. **Non utilisé en V1** (biais
    connu sur les étiquettes) ; remplacé par un `choice` à deux options
    neutres.
- **Checkpoint** : **multilingue uniquement** (mmBERT-base, ~322M
  paramètres, contexte 1 024 tokens). Le checkpoint anglais est exclu :
  il s'effondre hors anglais tout en restant très confiant.
- **Rôle 1 — Alerting calibré** : les probabilités alimentent
  directement des seuils dans OpenObserve via `event_log` (D84), sans
  parsing de sortie LLM. Exemple : P(incident) > 0,8 sur une fenêtre
  → alerte (anti-spam N3).
- **Rôle 2 — Filtre avant `media_fuse`** : le LLM de l'org n'est appelé
  que sur les fenêtres signalées par `text_decide`, ce qui rend
  tenable le plafond d'appels par org et par jour (§4.5).
- **Traçabilité** : une décision est une distribution liée aux ids de
  ses entrées (segments, événements, légendes), au format de provenance
  D184. Aucun texte généré, donc rien à marquer « généré par IA »
  au-delà du libellé de la décision.
- **P6 inchangé** : `text_decide` décide et route, il n'émet aucune
  commande. Seul un flow déployé par l'humain avec `device_write`
  actionne (D128).
- **Le zéro-shot ne suffit pas** : sur des décisions métier, les
  checkpoints de base sont proches du hasard (0,34–0,36 de précision
  contre 0,46 pour la classe majoritaire, selon la carte modèle).
  L'usage en production suppose donc :
  1. un **jeu annoté par l'org** (fenêtres réelles, ~200 à 500
     exemples) ;
  2. un **fine-tuning** (job sur worker GPU, P2.7) ;
  3. une **calibration de température** par (type de question, nombre
     d'options) sur les données de l'org, car le modèle sort
     sur-confiant (ECE ~0,31 → ~0,11 après recalage selon la carte).
  Le modèle fine-tuné est versionné dans le registre existant (média
  `kind=model` + `ml_models`, D81/D100), comme les détecteurs du Lab.
- **Limites de schéma** : 20 options maximum par `choice` (au-delà, la
  précision chute) ; état tronqué au contexte du checkpoint, ce qui
  reste compatible avec des fenêtres de 10 à 30 s.
- **`act_probability` ignoré** (sans signal selon la carte) ; le seuil
  porte sur la probabilité de la réponse.
- **Inférence** : graphe ONNX unique (encodeur + tête) exportable
  depuis le checkpoint HF (script `export_onnx.py` de
  [`receptron/laya`](https://github.com/receptron/laya), MIT). Exécution
  en Rust, sans Python ni Node. Le formatage d'entrée (marqueurs
  d'options, sérialisation JSON) doit reproduire l'implémentation de
  référence, vérifié par un test de non-régression sur les logits
  (voir Q12).
- **Ressources** : ~1,3 Go de poids en fp32 pour le multilingue
  (estimation : ~322M paramètres × 4 octets ; la carte HF indique
  ~647 Mo, sans doute en fp16), à mesurer. Les quelques centaines de ms
  annoncées par appel sur CPU sont à confirmer sur Pi 5 8 Go ; profil
  recommandé : worker GPU ou version quantifiée (Q13).

### 4.6 Vision Lab — génération de modèles de reconnaissance d'objets

**Besoin (Shan) :** l'utilisateur joue une vidéo ou ouvre plusieurs
images, entoure l'objet à détecter sous plusieurs angles et sur
plusieurs images différentes, et PNEX génère un modèle à partir de ça.

C'est la décision explicite attendue par la roadmap (décision #7 :
« besoin réel de fine-tuning détecteurs ») ; le « Model Lab » de
l'axe C démarrait par linfa — ce PRD en ouvre le volet vision.

#### 4.6.1 Parcours utilisateur

1. **Créer un projet Lab** et nommer les classes, par exemple
   « vanne_rouge » ou « casque_chantier ».
2. **Importer des médias** : plusieurs images, une ou plusieurs vidéos
   (bibliothèque média D21), ou un extrait d'une source existante
   (segments `video_segments` d'une caméra déjà branchée, D79).
3. **Annoter dans PNEX** :
   - Images : dessiner des boîtes (bounding boxes) et attribuer une
     classe.
   - Vidéo : lecteur intégré (celui de `/cameras`, D105), pause, boîte
     dessinée sur la frame.
     - **Propagation assistée** : PNEX suit la boîte sur les frames
       suivantes par interpolation entre keyframes (V1) puis par tracking.
       L'utilisateur corrige quand ça dérive.
     - L'utilisateur peut aussi n'annoter qu'une frame sur N et laisser
       l'interpolation faire le reste.
   - **Guidage de diversité** (M4) : le Lab signale les manques, par
     exemple « peu d'angles différents », « peu d'images en faible
     luminosité » ou « classe X sous-représentée ».
   - **Images négatives** : frames sans l'objet, pour réduire les faux
     positifs.
4. **Pré-annotation** (optionnelle, M4) : une fois un premier modèle
   entraîné, il propose des boîtes sur les nouvelles images et
   l'utilisateur valide ou corrige.
5. **Lancer l'entraînement** en un clic, avec des presets :
   - **Edge** : YOLOX-nano / tiny, 416 px, pour le Pi.
   - **Standard** : YOLOX-s.
   - **Précis** : YOLOX-m, pour le worker GPU.
6. **Suivre l'entraînement** : progression, courbes de loss et mAP, ETA,
   annulation possible.
7. **Évaluer** :
   - métriques par classe ;
   - galerie des erreurs (faux positifs, faux négatifs) ;
   - **test sur une vidéo** de validation directement dans le lecteur
     (même modal que le test live D104).
8. **Publier le modèle** dans le registre existant (nouvelle version du
   média `kind=model` + `ml_models`), qui passe la **vérification D100**
   comme tout ONNX importé, puis le **référencer** dans un ou plusieurs
   nœuds `vision_detect`.
9. **Boucle d'amélioration (active learning, M4)** : en production, les
   détections à faible confiance sont collectées dans une file « à
   revoir » du Lab. L'utilisateur les annote, ré-entraîne et publie une
   nouvelle version.

#### 4.6.2 Règles techniques du Lab

- **Split train/val par source** (vidéo ou session d'import), jamais
  par frame, pour éviter la fuite de frames quasi identiques entre train
  et val.
- **Déduplication** des frames extraites (similarité perceptuelle).
- **Fine-tuning** systématique depuis les poids COCO pré-entraînés.
- Augmentations YOLOX par défaut (Mosaic, MixUp, coupure en fin
  d'entraînement).
- **Seuils minimaux** avant de pouvoir lancer l'entraînement (N images
  annotées par classe, M images de validation) : valeurs indicatives,
  avertissement si elles ne sont pas atteintes.
- **Export ONNX** automatique en fin d'entraînement, à taille d'entrée
  fixe (D100 lit les dimensions dans le fichier). Quantification INT8
  optionnelle pour l'edge, **sous réserve que tract l'exécute** (à
  vérifier au M4).
- Format interne compatible **COCO**. Import et export COCO pour
  l'interopérabilité (pas un parcours principal, P1).

#### 4.6.3 Registre et versioning des modèles

On **réutilise** le registre existant au lieu de créer des tables
`model` / `model_version` :

- les octets ONNX sont une **nouvelle version** du média `kind=model`
  (D21 : versioning, stockage fs/S3 **RustFS**,
  purge) ; `ml_models` porte la spec d'inférence (labels, taille,
  seuils) et peut épingler une version (D81) ;
- nouvelles tables append-only (école `flow_versions`, D18) :
  - `lab_datasets` / `lab_dataset_versions` : snapshot figé des
    annotations (référence des médias + boîtes), **versionné** ;
  - `lab_training_runs` : dataset version, preset, hyperparamètres,
    image d'entraînement (digest), métriques, version de média produite,
    checksum (sha256 du média) ;
- **Enregistrer** (nouvelle version du média) reste séparé de
  **référencer** (un nœud ou `ml_models` épingle la version). Le rollback
  consiste à ré-épingler une version antérieure puis redéployer le flow.
- Tout modèle est reproductible depuis `lab_training_runs` (dataset
  figé + hyperparamètres + digest de l'image).

### 4.7 Postures / postes de travail

> **À clarifier (question ouverte Q1)** : « reconnaissance object/poste »
> signifie soit **pose** (estimation de posture humaine), soit **poste de
> travail** (surveillance d'un poste industriel).

- **Si poste de travail** : c'est couvert par les zones (§ 4.3) et les
  classes custom (opérateur, outil, pièce), avec des événements du type
  « opérateur présent au poste », « pièce absente » ou « durée de cycle ».
  Un poste ou un cycle est une **plage** (D169/D182).
- **Si pose humaine** : YOLOX ne fait pas d'estimation de pose. Il
  faudrait un modèle dédié sous licence permissive (candidats à vérifier :
  RTMPose, MoveNet — annoncés Apache-2.0), nouvelle famille dans
  `ml_models`, en parallèle de la contrainte « YOLOX exclusivement », qui
  vaut pour la détection d'objets.

### 4.8 Studio de rapports intégré

- **Conflit avec l'existant** : le studio de rapports est **reporté**
  (D173 : rien ne sort de PNEX ; D174 : pas de synthèse IA en V1 ; D3 :
  les rapports planifiés passent par OpenObserve) et l'ontologie prévoit
  des rapports liés aux objets (D187). M5 exige donc une décision
  explicite qui rouvre D173/D174 et précise D3, avec les garde-fous de
  `media-ingest.md` §15 (échappement, rendu PDF sans réseau, aucune
  publication automatique d'une synthèse IA, relecture humaine).
- L'utilisateur reste dans PNEX (P1).
- Sources du rapport : descriptions et résumés vidéo, transcriptions,
  événements, données IoT corrélées sur la même plage temporelle.
- Fonctions :
  - timeline interactive (média + événements) ;
  - recherche plein texte (endpoint transcripts D165, événements D84) et
    par classe d'objet ;
  - extraits (clip ou frame) intégrés au rapport ;
  - génération assistée par l'agent PNEX (LLM de l'org) ;
  - export (PDF ou Markdown).

### 4.9 Intégration au moteur de flows

Nœuds natifs Rust, schéma config/ports dans `pnex-core`, **kinds en
snake_case** comme l'existant :

| Nœud | État | Entrée → Sortie |
|---|---|---|
| `camera_source` (`source: device \| stream`, D175) | existant, étendu par D175 | device ou flux → frames horodatées (bus caméra) |
| `media_source` (D163) | proposé (P2.13) | flux → segments transcrits (texte) |
| `vision_detect` (D83) | existant | frames + modèle du registre → détections |
| `vision_track` | **nouveau** | détections → tracks (ByteTrack) |
| `vision_zone` | **nouveau** | tracks + polygones → événements de zone |
| `vision_caption` | **nouveau** | frames ou crops → légendes (endpoint VLM de l'org) |
| `text_decide` | **nouveau** | segments / événements / légendes / description → décisions typées calibrées (`choice`, `score`) → `event_log` |
| `media_fuse` | **nouveau** | segments + événements + légendes → description horodatée |

- École D78 : le flow transporte des **références et du texte**, jamais
  de pixels ni d'audio en masse ; le calcul lourd (VLM, fusion LLM) est
  un appel sortant borné ou un job, pas une boucle dans le runtime.
- Les événements vont dans **OpenObserve** via `event_log` (D84), donc
  exploitables par l'alerting existant (anti-spam N3), par exemple
  « personne sans casque en zone X pendant plus de 10 s ».
- **Livrable du même commit pour chaque nouveau nœud** : `NodeDoc` dans
  `pnex-core/src/flow/node_docs.rs` (garde `every_kind_is_documented`),
  fiche `assistant-kb`, clés i18n des statuts (`flow-status-*`) et codes
  d'erreur dans `err_codes::ALL` + les deux `.ftl`.

---

## 5. Architecture

```mermaid
flowchart LR
  CAM["Caméras IP / ESP32-CAM"] --> SRC
  RADIO["Radio / audio"] --> SRC
  FILE["Fichiers (bibliothèque média D21)"] --> SRC
  subgraph PNEX["PNEX (Loco + Dioxus)"]
    SRC["media_streams / camera_source<br/>(D159, D175)"] --> ASR["ASR pnex-asr<br/>(job transcribe_segment)"]
    SRC --> DET["vision_detect<br/>(pnex-vision, tract)"]
    DET --> TRK["vision_track"] --> ZONE["vision_zone"]
    SRC --> CAP["vision_caption<br/>(VLM de l'org)"]
    ASR --> FUSE["media_fuse<br/>(LLM de l'org)"]
    ZONE --> FUSE
    CAP --> FUSE
    ZONE --> O2["OpenObserve<br/>ev_*, tx_*, camera_detections"]
    FUSE --> O2
    O2 --> OUT["Alerting · Dashboards · Synoptiques · Rapports (M5)"]
    LAB["Vision Lab (UI Dioxus)<br/>annotation, propagation, évaluation"] --> Q["Queue Loco<br/>(tag train réglable)"]
  end
  Q --> TRAIN["Worker train<br/>(image YOLOX figée, Python isolé)"]
  TRAIN --> REG["Bibliothèque média kind=model<br/>(RustFS) + ml_models"]
  GPU["Worker GPU (fabric P2.7)<br/>VLM, LLM local, ASR large, entraînement"] -.-> TRAIN
```

- **Cœur** : Loco (API, orchestration, queue Loco sur PostgreSQL) et Dioxus
  (UI Lab, lecteur, annotation, studio).
- **Stockage** :
  - Postgres : config, sources, projets Lab, annotations, runs ;
  - OpenObserve : événements, transcriptions, calques de détection ;
  - RustFS / fs (bibliothèque média D21) : médias bruts, frames de
    dataset, artefacts ONNX.
- **Entraînement** (paragraphe d'origine, **remplacé par D204** : pas de Python, ré-apprentissage de la tête en Rust dans le process API) :
  - Job `train_detector` dans la **queue Loco**, même mécanique que
    `stitch_panorama` / `transcribe_segment` (claim idempotent dans la
    ligne de domaine, état terminal écrit par le worker, timeout,
    annulation). Tag réglable `PNEX_TRAIN_QUEUE_TAG` (règle D166) ; il
    devient la capability `feature:train` + GPU quand la fabric (P2.7)
    livrera `required_capabilities`.
  - **Exception assumée à « pas de Python côté plateforme »**
    (`ml-vision.md`) : l'entraînement est le **seul** composant Python,
    confiné à une image Docker YOLOX **figée** (PyTorch, onnx,
    pycocotools épinglés, digest consigné dans le run), **hors** de
    l'image serveur (distroless), lancée par le worker sans réseau
    sortant, avec pour seules entrées le snapshot du dataset et pour
    seule sortie l'ONNX + les métriques. Le runtime d'inférence reste en
    Rust.
- **Inférence détection** : `pnex-vision` (tract), dans le process du
  runtime de flows.
- **VLM / LLM** : endpoint OpenAI-compatible **de l'org** (D119) — vLLM
  local sur worker GPU ou API distante —, appel filtré R8.

### Profils de déploiement

| Profil | Capture | ASR | Détection | VLM / LLM | Entraînement |
|---|---|---|---|---|---|
| **Pi seul** | ✅ | léger (sherpa-onnx CPU) | YOLOX nano/tiny, faible fps | API de l'org | ❌ (ou worker distant) |
| **Pi + worker GPU** | ✅ | large | s/m | local (vLLM) | ✅ |
| **SaaS** | ✅ | large | s/m, scalé | local ou API de l'org | ✅ pool scalé |

---

## 6. Exigences non fonctionnelles

- **Licences** : 100 % permissives au runtime (MIT/BSD/Apache, LICENSE
  lu). Licence de chaque modèle et de chaque poids vérifiée, affichée à
  l'import (champ SPDX obligatoire, D167) ; la palette signale les
  licences non commerciales.
- **Performance edge** : détection nano/tiny exploitable sur Pi 5 à
  faible fps ; débit soutenable affiché (D101). Accélérateur optionnel.
- **Ressources** : la stack au repos reste sous la cible actuelle
  (< 1 Go). Modules média et vision **activables à la demande**.
- **Confidentialité** : médias et datasets restent chez l'utilisateur en
  self-hosted. L'usage d'une API distante (VLM, LLM) est explicite et
  configurable par source.
- **Sécurité** (R1–R20) : org depuis le principal (R1), garde de rôle +
  test viewer → 403 sur chaque écriture (R2), datasets et runs isolés
  par org, aucun identifiant de stockage dans les DTO (R4, R16), clés de
  stockage construites côté serveur (R18), appels VLM/LLM filtrés (R8),
  image d'entraînement sans réseau. Revue §6 avant chaque commit.
- **Rétention** : politique séparée pour les médias bruts (lourds) et
  les événements (légers).
- **Reproductibilité** : tout modèle est reconstructible depuis son
  snapshot de dataset, ses hyperparamètres et le digest de l'image.
- **Traçabilité** : chaque phrase de description renvoie à ses sources
  horodatées (D184).
- **Assistant IA** : chaque nouveau nœud, page (`/lab`) et code d'erreur
  livré avec sa `NodeDoc` et sa fiche KB ; si l'assistant gagne des
  outils Lab, règle §9.3 de `ai-assistant.md` (service partagé,
  `expected_version`), et jamais de lancement d'entraînement ni de
  déploiement par l'assistant (D144).

---

## 7. Phasage proposé

Les lots audio sont ceux de `media-ingest.md` ; ce PRD ne les duplique pas.

| Phase | Contenu | Livrable |
|---|---|---|
| **M0 — Socle média** | **Terminé** : P2.13 lots 1–6 (capture flux/vidéo/audio → texte, `media_source`, D175 caméras IP sur le bus) + D206 (fichier de la médiathèque comme source) | Transcription horodatée d'une vidéo ou d'une caméra IP |
| **M1 — Détection** | `vision_track` (ByteTrack Rust), `vision_zone` (éditeur de polygones sur l'image), événements dérivés via `event_log` + alerting (`vision_detect` existe déjà) | Événements objets en zone + alertes |
| **M2 — Vision Lab v1** | Projets, import images/vidéos, annotation boîtes, interpolation vidéo, split par source, `lab_datasets` versionnés, job `train_detector` (image YOLOX figée), export ONNX → registre existant + check D100 | Modèle custom créé de bout en bout dans PNEX |
| **M3 — Description** | `vision_caption` (VLM de l'org), `media_fuse`, résumé global, provenance D184 ; décision sur D174 (usage interne) | Description vidéo horodatée |
| **M3bis — Décision rapide** | `text_decide` (Laya multilingue, ONNX), POC sur ~200 fenêtres réelles annotées : zéro-shot vs fine-tuné vs LLM de l'org, calibration de température, puis branchement en filtre devant `media_fuse` | Alerte calibrée sur fenêtre + réduction mesurée des appels LLM |
| **M4 — Lab v2** | Pré-annotation, active learning (file « à revoir »), guidage de diversité, tracking pour la propagation, INT8 | Boucle d'amélioration continue |
| **M5 — Rapports (D207–D210)** | Pas de studio : pages écrites par des nœuds génériques (`llm_text`, `page_write`, `page_pdf`, pièce jointe `pnex_notify`), approbation humaine par défaut — **livré sur la branche le 2026-10-13** | Rapport généré sans sortir de PNEX |

Dépendances : fabric de workers P2.7 (GPU distant ; contournable par un
process `--worker=train` joint en mesh, comme D166) ; P2.13 lot 6 pour
M0 (lot 1 livré) ; ontologie L4 (D182 plages, D187 rapports) pour M5.
M3bis dépend de M0 (segments transcrits) et, pour le fine-tuning, de
la fabric P2.7 ; le job d'entraînement relève de la même exception
Python confinée que `train_detector` (§11, point 10).

---

## 8. Indicateurs de succès

- **Temps du premier modèle custom** (import → modèle référencé dans un
  flow) : moins d'une heure pour un utilisateur non technique, hors
  temps de calcul.
- mAP@0.5 sur la validation du Lab, affichée et comparée entre versions.
- Pourcentage de phrases de description avec au moins une source
  traçable : cible 100 %.
- Démarrage du profil Pi seul sans GPU, avec transcription et détection
  fonctionnelles.
- Contenus communautaires du type « projet d'un week-end » (Instructables,
  Medium) bâtis sur le Lab.

---

## 9. Hors périmètre (v1)

- Tout nœud vision qui commande un actionneur (P6). L'alerting et la
  notification restent possibles ; le câblage d'une détection vers
  `device_write` par un flow humain reste régi par D128 (voir Q9).
- Reconnaissance faciale, identification de personnes, identification
  vocale (cohérent avec `media-ingest.md` §2).
- Segmentation d'instances ; entraînement autre que la détection YOLOX ;
  entraînement de modèles ASR.
- Annotation collaborative multi-utilisateur temps réel.
- Publication de rapports hors de PNEX tant que D173 n'est pas rouverte.

---

## 10. Questions (toutes tranchées au 2026-10-13)

| # | Question |
|---|---|
| Q1 | **Tranchée (2026-10-13)** : **poste de travail** (zones D193 + classes du Lab + plages D182). Pose humaine hors V1 ; si elle revient, famille `ml_models` distincte, licence relue. ~~« Reconnaissance poste » : **pose humaine** ou **poste de travail** ? Si pose : quel modèle sous licence permissive (RTMPose, MoveNet à vérifier) ?~~ |
| Q2 | **Tranchée (2026-10-13)** : tract reste le seul runtime (confirmé par D197) ; `ort` seulement si un bench chiffré le justifie, derrière une feature de build, pour le nœud concerné. ~~Inférence : tract (en production) suffit-il pour YOLOX-s/m et l'INT8, ou faut-il ajouter `ort` ? Burn reste à benchmarker (`ml-vision.md` étape 1).~~ |
| Q3 | **Tranchée (2026-10-13)** : interpolation linéaire entre keyframes en V1 ; propagation par `vision_track` en M4 ; pas de tracker visuel sans détecteur. ~~Propagation des boîtes dans le Lab avant tout modèle : interpolation seule (V1) ou suivi visuel dédié (tracker sans détecteur) ?~~ |
| Q4 | **Tranchée (2026-10-13, révisée par D204)** : l'entraînement est un ré-apprentissage de la tête en Rust, sur CPU, Pi compris ; plus de presets ni d'image GPU. ~~Entraînement sans GPU (CPU, Pi) : interdit, ou autorisé en mode « petit dataset / nano » avec avertissement ?~~ |
| Q5 | **Tranchée (2026-10-13)** : aucun VLM livré ni imposé (D119) ; la doc cite des modèles à licence relue à la rédaction, SPDX affiché à la configuration. ~~VLM par défaut : aucun (D119, l'org choisit) — quels modèles recommander dans la doc, licence vérifiée ? ASR : tranché au lot 0 (`media-asr-bench.md`).~~ |
| Q6 | **Tranchée (2026-10-13)** : `media_stream` = type système par adaptateur (D177) ; zones (config du nœud), projets et jeux du Lab restent dans leurs tables en V1. ~~Place des sources, modèles, datasets et zones dans l'ontologie 0.2.0 (types système D177, plages D182).~~ |
| Q7 | **Tranchée (2026-10-13)** : images et jeux du Lab comptent dans le stockage média de l'org ; quota = réglage plateforme, aucun plafond codé en self-hosted ; rétention séparée brut / événements (§4.1). ~~Rétention et stockage des médias bruts et datasets en SaaS : coût et quotas par tenant.~~ |
| Q8 | ~~Diarisation dès M0 ?~~ — **déjà planifiée** : lot 5 de P2.13 (D166), labels locaux. |
| Q9 | **Tranchée (2026-10-13, Shan)** : oui, régi par D128 — seul un flow déployé par un humain agit ; aucune porte de deploy spécifique à la vision. ~~Une détection peut-elle alimenter `device_write` via un flow humain (D128 le permet aujourd'hui), ou ce PRD l'interdit-il (porte de deploy) ?~~ |
| Q10 | **Tranchée (2026-10-13, Shan)** : pas de gel ni d'attente de tag ; la 0.1.0 est épinglée, la 0.2.0 dira ce qu'elle livre, en beta. ~~Version cible : après la 0.2.0, ou exception au gel comme P2.13 (décision #17) ?~~ |
| Q11 | **Tranchée (D194)** : usage interne autorisé, marqué IA, aucune publication hors de PNEX. ~~D174 : la description par fenêtre (M3) est-elle autorisée en usage interne, et sous quelles règles (marquage IA, relecture) ?~~ |
| Q12 | **Tranchée (D197)** : tract suffit (écart ≤ 2e-5, 160–250 ms/question x86). ~~Inférence `text_decide` : tract supporte-t-il le graphe ModernBERT/mmBERT exporté, ou ce nœud justifie-t-il `ort` (cf. Q2) ? Le portage Rust du formatage d'entrée doit égaler la référence Python à 1e-4 près.~~ |
| Q13 | **Tranchée (2026-10-13)** : fp32 non proposé sur Pi ; INT8 obligatoire sur Pi, calibration re-mesurée sur le modèle quantifié ; sinon worker. Mesure Pi 5 à faire. ~~`text_decide` sur Pi : fp32 acceptable sur Pi 5 8 Go, ou quantification INT8 obligatoire (précision et calibration à re-mesurer) ?~~ |
| Q14 | **Tranchée (2026-10-13)** : seuil retenu ≥ 80 % de précision sur le jeu annoté de l'org et ECE ≤ 0,10 après calibration ; à appliquer au deploy quand le fine-tuning existera. ~~Seuil de mise en production de `text_decide` : quel écart de précision avec le LLM de l'org est acceptable en échange d'un gain de latence et de coût ? Proposition : ≥ 80 % de précision sur le jeu annoté de l'org et un ECE ≤ 0,10 après calibration.~~ |
| Q15 | **Tranchée (D197)** : révision HF épinglée `7b928d82`, script d'export vendu (`scripts/laya/`). ~~Maturité de l'écosystème Laya : modèle récent, benchmarks comparatifs auto-publiés, portage `receptron/laya` à un seul auteur (oct. 2026). Faut-il vendorer le script d'export et figer une révision du checkpoint (hash HF) plutôt que suivre `main` ?~~ |

---

## 11. Journal de relecture (2026-10-10)

Corrections apportées à la v0.1 après lecture de la doc existante :

1. **Stockage** : le stockage passe par la bibliothèque média D21
   (fs ou S3 RustFS).
2. **`ort` → tract** : l'inférence YOLOX tourne déjà avec tract dans
   `pnex-vision` (D82) ; `ort` n'est qu'un repli (Q2).
3. **Nœuds** : `media.source` / `vision.detect` renommés selon les kinds
   existants (`camera_source`, `media_source`, `vision_detect`,
   snake_case) ; seuls `vision_track`, `vision_zone`, `vision_caption`,
   `media_fuse` sont nouveaux.
4. **Sources média et ASR** : déjà spécifiés par `media-ingest.md`
   (D159–D175) ; faster-whisper (stack Python) remplacé par `pnex-asr`
   (sherpa-onnx / whisper.cpp, lot 0 mesuré).
5. **Registre de modèles** : pas de nouvelles tables `model` /
   `model_version` ; réutilisation du média `kind=model` + `ml_models`
   (D81, D100, D167), ajout de `lab_datasets(_versions)` et
   `lab_training_runs` pour la reproductibilité.
6. **LLM « GLM 5.3 Flash par défaut »** retiré : pas de LLM plateforme
   (D119), le LLM et le VLM sont ceux de l'org.
7. **P6 reformulé** : la frontière d'actionnement existe déjà (D128 :
   seul un flow déployé par l'humain agit) ; « aucune logique d'action »
   était inexact puisque `device_write` existe (Q9).
8. **Geofencing** : il n'existe pas encore ; les zones caméra sont un
   concept nouveau en coordonnées image.
9. **Studio de rapports / description IA** : en conflit avec D173/D174
   (reportés) et D3 ; signalé, décisions Q10–Q11 et préalable de M5.
10. **Python** : l'entraînement contredit « pas de Python côté
    plateforme » (`ml-vision.md`) ; exception confinée et explicite, et
    réponse à la décision #7 de la roadmap.
11. **Version cible** : « 0.2.x » contredit le gel des piliers pendant la
    0.2.0 (P2.14) ; cible après 0.2.0 sauf exception (Q10).
12. **Q8 (diarisation)** : déjà au lot 5 de P2.13.
13. Diagramme ASCII converti en Mermaid (convention des docs).
14. Ajout des exigences sécurité (R1–R20) et assistant IA (NodeDoc, KB),
    obligatoires pour tout nouveau nœud.
15. **Ajout §4.5bis (2026-10-12)** : étage de décision rapide
    `text_decide` (Laya multilingue, Apache-2.0, non génératif) en
    filtre devant `media_fuse` et en source d'alertes calibrées. Les
    limites tirées de la carte modèle (zéro-shot faible, `noul` biaisé,
    sur-confiance, `act_probability` inutilisable) sont intégrées comme
    règles. Questions Q12–Q15 ajoutées, phase M3bis ajoutée.

---

## 12. Décisions actées et état

| # | Décision | Date |
|---|---|---|
| **D192** | **`vision_track`** : suivi par source (caméra device ou flux), ByteTrack réimplémenté en Rust sans dépendance (MIT, aucun code repris) : prédiction à vitesse constante du centre (lissée 0,5) au lieu d'un Kalman complet, association en deux étapes (détections ≥ `high_score` sur tous les suivis, puis détections faibles sur les suivis restants, sans jamais en ouvrir), par classe, par IoU ≥ `min_iou`, **appariement glouton** (meilleur IoU d'abord) au lieu du hongrois — suffisant pour les scènes clairsemées d'une caméra, à revoir si des foules échangent les identifiants. Suivi provisoire jusqu'à `min_hits` correspondances (puis `appear`), supprimé sans bruit s'il rate une image ; suivi confirmé perdu après `lost_after_secs` (`disappear`, avec `t0_ms`, `t1_ms`, `duration_s`, `conf_avg`). Port 0 = suivis courants de la source (à chaque image, **et** quand des suivis expirent sans nouvelle image : balayage horloge murale 1 s, nécessaire puisque `vision_detect` en `on_detection` n'émet rien quand la scène est vide) ; port 1 = événements. Identifiants propres au nœud, remis à zéro au redéploiement. Pas de statut D103 (nœud de calcul pur, aucun chargement ni dépendance externe). Défauts réglés pour 1 fps : `high_score` 0,5, `min_iou` 0,2, `min_hits` 2, `lost_after_secs` 3. | 2026-10-12 |
| **D193** | **`vision_zone`** : polygones en **coordonnées image normalisées 0–1** (survivent à un changement de résolution), 1 à 16 zones de 3 à 32 sommets, noms uniques ; testés sur un **point d'ancrage** du cadre (bas-centre par défaut : pieds, roues ; ou centre). Entrée après `min_dwell_secs` de présence continue (« présent depuis plus de N s », 0 = immédiat), sortie quand l'ancrage quitte la zone **ou que le suivi disparaît** ; seule une entrée émise peut sortir (visite courte = silence). Un message par événement `{event: enter \| exit, zone, track_id, label, ts_ms, t0_ms, duration_s (sortie), count}`, `count` = occupation de la zone juste après l'événement (comptage sans nœud dédié). Les zones s'appliquent à **toutes** les sources qui arrivent au nœud (un nœud par point de vue) ; `preview_device` sert seulement de fond à l'éditeur. Éditeur dans l'inspecteur du nœud : vue live de la caméra (D75) à son propre ratio, clics → sommets, aucun stockage d'image. Événements vers `event_log` (O2 `ev_*`, D84) et alerting existant ; aucune commande (P6/D128). | 2026-10-12 |

| **D194** | **Q11 / D174 tranchée pour l'usage interne** : les descriptions IA de M3 (légendes, description par fenêtre) sont autorisées **dans PNEX** (flows, événements, tableaux de bord), toujours marquées `ai_generated: true` et libellées « générée par IA » ; aucune publication hors de PNEX (D173 inchangée), aucune action sur une sortie IA (P6). Les rapports exportés (M5) restent à décider. | 2026-10-13 |
| **D195** | **`vision_caption`** : le runtime n'envoie qu'une **référence** (`frame_key`) à la route interne `/internal/flow/ai/caption` (jeton de service + fencing D106) ; le backend relit l'image sur le bus caméra (clé préfixée par l'org, sinon 404), la recadre éventuellement (meilleure détection ou suivi, marge 10 %), la réduit à 1024 px et appelle le **fournisseur LLM par défaut de l'org** (D119) avec l'image (blocs image Anthropic / `image_url` OpenAI-compatible) ; la clé ne quitte jamais le backend (R6), l'appel sort par le client filtré R8. Une description par source au plus toutes les `min_interval_secs` (10 s par défaut) ; câbler le nœud **est** le choix explicite d'envoyer l'image au fournisseur (texte d'avertissement dans l'inspecteur). Pas de VLM plateforme ni de modèle par défaut (Q5 : la doc recommandera des modèles à poids ouverts servis en local). | 2026-10-13 |
| **D196** | **`media_fuse`** : le nœud normalise ses entrées en éléments `{id, kind, ts_ms, source, text}` (`detection` = événements de détection/suivi/zone, `transcript` = segments ASR, `caption` = légendes) et envoie chaque fenêtre (30 s par défaut, ≤ 200 éléments, textes ≤ 2000 caractères) à `/internal/flow/ai/fuse`. Le prompt impose la hiérarchie de confiance (détections = faits, ASR = parole, légendes = indicatives), interdit d'inventer, traite les textes comme des **données délimitées** (aucun outil) ; la réponse JSON est filtrée côté serveur : **une phrase dont une source n'est pas un id de la fenêtre est rejetée** (compteur `rejected`). **Plafond commun** `vision_caption` + `media_fuse` : 2000 appels par org et par jour UTC (`settings.ai.flow_daily_calls`, compteur Valkey) → 429 `flow-ai-quota`. Statut de nœud D103 `ai-call-failed` avec le code serveur en détail. | 2026-10-13 |

| **D197** | **`text_decide` (Laya)** — Q12 tranchée : **tract suffit**, pas d'`ort`. Mesures 2026-10-13 (x86, release) : checkpoint multilingue (mmBERT-base) exporté depuis la révision HF **épinglée** `7b928d82` (Q15), écart de logits tract / ONNX Runtime ≤ 2e-5 (exigence 1e-4), ids de tokens identiques à la référence Python (cas français compris), 160–250 ms par question sur CPU (ORT 70–150 ms), chargement + optimisation 3,4 s, ~1,3 Go de poids fp32. Le **bundle publié `receptron/laya-onnx` est le checkpoint anglais** (exclu, §4.5bis) : le bundle multilingue se construit avec `task laya:bundle` (outillage hors ligne uv + torch CPU, export MIT adapté ; PneX n'exécute jamais Python). Les jetons spéciaux viennent de `tokenizer_config.json` comme dans `rl_common.py` (le portage TS les code en dur `[CLS]`…, faux pour le multilingue). Inférence **dans le backend** (crate `pnex-decide`, une copie du modèle par serveur, chargement paresseux, un calcul à la fois), jamais dans le runtime d'org ; route interne `/internal/flow/ai/decide` (jeton + fencing, questions re-vérifiées). Bundle installé par l'admin (`settings.decide.model_dir` / `PNEX_DECIDE_MODEL_DIR`), absent → 503 `decide-unavailable`. Questions `choice` et `score` seulement (`noul` refusé, §4.5bis) ; état = texte (`text_field`, défaut `text`) : les objets JSON sont sérialisés clés triées (raccourci assumé, la référence garde l'ordre d'insertion). Le checkpoint multilingue est **non calibré** (températures 1,0) : l'UI le dit ; calibration et fine-tuning par org restent le POC M3bis. | 2026-10-13 |

| **D198** | **Vision Lab — projets et annotation** : un projet = des classes **positionnelles** (une boîte référence l'index de sa classe ; renommer sur place ou ajouter à la fin, ne retirer que les dernières et seulement si aucune boîte ne les utilise → 409 `lab-class-in-use`) ; images = des **assets de la bibliothèque média** (D21, JPEG/PNG vérifiés à l'import par lecture de l'en-tête, jamais copiés) ; boîtes normalisées 0–1 en jsonb sur l'image, ≤ 200, marquage « sans objet » (exemple négatif) exclusif des boîtes ; concurrence optimiste par `version` (mise à jour conditionnelle atomique, 409 `lab-annotations-conflict`). **Split train/val par source** (§4.6.2) : un import = une source, répartie par hachage FNV-1a stable (≈ 20 % en validation), forçable image par image. Avertissements indicatifs, jamais bloquants : < 20 images par classe, < 5 images de validation, aucun négatif. UI `/lab` + `/lab/:id` : sélecteur multiple de photos (un import = une source, d'où l'abandon du sélecteur de ressources à choix unique), dessin à la souris sur l'image au ratio réel, couleurs par classe, raccourcis 1–9 / Suppr / ← → / N, enregistrement optimiste sérialisé par image, statistiques calculées localement par la même fonction que le serveur. Lecture pour tous les rôles, écriture owner/admin/member (R2). | 2026-10-13 |
| **D199** | **Versions de jeu de données** : `lab_dataset_versions` append-only (école D18) ; une version **fige** les images annotées, leurs boîtes, le split effectif et la **version média** de chaque image (une image remplacée dans la médiathèque ne change pas un jeu figé) ; numéros consécutifs sous verrou de la ligne projet. C'est ce que liront l'export et, plus tard, `lab_training_runs` (M2b). | 2026-10-13 |
| **D200** | **Export COCO et report de l'entraînement** : export d'une version en zip (`images/<id>.<ext>` aux versions figées, `annotations/train.json` et `val.json` au format COCO détection, boîtes en pixels, catégories 1..N dans l'ordre des classes, `classes.txt`), construit en mémoire (raccourci assumé : ≤ 3000 images). Téléchargeable par un lecteur. **L'entraînement dans PNEX (job `train_detector`, image YOLOX Python figée, M2b) est reporté** : il demande une image de plusieurs Go, idéalement un worker GPU (P2.7) et des choix à valider (Q4 entraînement CPU). D'ici là, le parcours est : Lab → export COCO → entraînement YOLOX hors PNEX → ONNX importé dans **Modèles** (vérifié D100) → `vision_detect`. | 2026-10-13 |

| **D201** | **Pré-annotation dans le Lab** (anticipée de M4) : `POST /api/v1/lab/images/{id}/suggest {model_id, min_score}` exécute un modèle de détection du registre de l'org (cache et pool vision existants, D82/D104) sur l'image et renvoie des **propositions sans rien écrire** ; un libellé égal à un nom de classe (casse, espaces et tirets ignorés) prend cette classe. UI : propositions en pointillés, « Garder les N propositions reconnues » ne garde **que** les propositions mappées (une proposition grise ne devient boîte que par un clic explicite, jamais en masse), « Écarter » efface le reste. Réservé aux rôles d'écriture (calcul sur le pool partagé). Avec un premier modèle custom, c'est la boucle d'amélioration du §4.6.1 étape 4. | 2026-10-13 |

| **D202** | **Images d'un enregistrement de caméra dans le Lab** (premier pas du parcours vidéo du §4.6.1, étape 2) : `POST /api/v1/lab/projects/{id}/import-recording {device, from, to, every_secs}` prend dans les segments MJPEG-AVI (D79) la trame la plus proche de chaque instant `from + k·N s` (aucun décodage ni ré-encodage : la trame est déjà un JPEG), la range comme `photo` de la médiathèque (chemin d'écriture partagé `write_version`) et l'ajoute au projet ; **un import = une source**, donc un extrait vidéo entier tombe du même côté du split. Bornes : 1 h d'extrait, 200 images, intervalle 0,5–600 s ; plage sans enregistrement → 404 `camera-export-empty`. UI : caméra, jour et heures **locales**, intervalle, estimation du nombre d'images avant import. Restent pour la vidéo : fichiers vidéo uploadés (ffmpeg), interpolation de boîtes entre trames clés (M2 complet), flux IP (D175). | 2026-10-13 |
| **D203** | **Vidéos dans le Lab et interpolation des boîtes** (§4.6.1 étape 3, Q3) : la médiathèque accepte le kind `video` (MP4 / MOV reconnus par la marque `ftyp` + extension, servis inline). `POST /api/v1/lab/projects/{id}/import-video {asset_id, from_secs, to_secs, every_secs}` décode la version courante avec le **décodeur confiné de D160** (même bac à sable, pipe seulement, décodeurs h264/hevc/mjpeg ; `trim` + `fps` + JPEG en sortie), mêmes bornes que D202 (≤ 1 h, ≤ 200 images), fichier ≤ 512 Mo lu en mémoire. Un MP4 dont le `moov` est en fin de fichier (vidéos de téléphone) est remis en tête **en mémoire** avant décodage (décalage des `stco`/`co64`, raccourci : pas de passage `stco` → `co64`, fichier > 4 Gio refusé). Toutes les imports d'une même vidéo partagent la source `video-<asset>` (jamais à cheval entre entraînement et validation). Chaque image garde sa **position dans la source** (`lab_images.frame_ms`, migration `000015` ; époque pour un enregistrement, depuis le début du fichier pour une vidéo). `POST /api/v1/lab/images/{id}/interpolate` propose, sans rien écrire, les boîtes **interpolées linéairement** entre l'image annotée précédente et la suivante de la même source ; appariement par classe, centres les plus proches d'abord, une boîte sans partenaire n'est pas proposée. Éditeurs seulement (comme D201). Codes `lab-video-unsupported`, `lab-video-too-large`, `lab-video-decode-failed`, `lab-interpolate-no-keyframes`. | 2026-10-13 |
| **D204** | **Entraînement sans Python : ré-apprentissage de la tête YOLOX en Rust** (Shan, 2026-10-13 : « python free toujours » ; remplace l'image YOLOX Python de D200/M2b). Le réseau d'un modèle YOLOX **déjà enregistré** (COCO ou un modèle du Lab) reste figé ; seules ses dernières convolutions 1×1 par niveau (classes K, objectivité, boîte) sont ré-apprises (`pnex_vision::train`) : caractéristiques extraites par tract (image + miroir horizontal), affectation SimOTA allégée (dans la boîte et à 2,5 pas du centre, k dynamique par l'IoU des boîtes pré-entraînées), Adam sur BCE (objectivité pondérée pour les négatifs échantillonnés, classes) + L1 (boîte), puis réécriture des initialiseurs dans l'ONNX (K classes, cible du `Reshape`, sortie ; indices de forme intermédiaires retirés). L'ONNX obtenu passe la vérification D100 et entre dans **Modèles** (média `kind=model` + `ml_models`, libellés = classes du projet), mAP@0,5 mesurée en validation avec le `Detector` de production. Exécution **dans le process API**, une à la fois par pod, file d'attente par sémaphore, vivacité par battement (10 s ; > 60 s = `lab-train-interrupted`), annulation par la ligne. `POST /api/v1/lab/versions/{id}/train {base_model_id, epochs ≤ 300}`, `GET /api/v1/lab/projects/{id}/runs`, `POST /api/v1/lab/runs/{id}/cancel` ; table `lab_training_runs` (migration `000016`). Limite assumée : moins précis qu'un entraînement complet pour des objets très éloignés de COCO ; l'entraînement complet (Burn) n'est pas planifié. | 2026-10-13 |
| **D205** | **Boucle d'amélioration du Lab (M4)**. (1) **File « à revoir »** (active learning) : `vision_detect` gagne `review_project` (projet du Lab, vide = désactivé), une **bande de doute** `review_min`–`review_max` (0,25–0,6) et `review_every_secs` (60 s, 5–3600) ; une image dont un score candidat tombe dans la bande part **par référence** (`frame_key`) vers `/internal/flow/lab-review` (URL dérivée de `PNEX_FLOW_MODEL_URL`, jeton + fencing D106, envoi non bloquant). Le backend relit l'image sur le bus (clé préfixée par l'org), l'ajoute au projet avec `review = true` et les **propositions** du modèle (normalisées, classe reprise par nom, D201), source `review-<caméra>-<jour>` ; ≤ 500 images en attente par projet (429 `lab-review-full`). Une image sort de la file dès qu'elle a des boîtes ou est marquée sans objet. UI : point violet, filtre « Seulement les images à revoir », propositions affichées à l'ouverture. (2) **Propagation par suivi** : `POST /api/v1/lab/images/{id}/propagate {model_id, min_score}` = pré-annotation D201 sur l'image, puis chaque boîte de l'image annotée précédente de la source donne sa classe à la proposition qui la recouvre le plus (IoU ≥ 0,3, meilleurs couples d'abord), le reste est écarté ; sert quand le modèle ignore les noms de classes du projet. (3) **Guidage de diversité** : avertissements `lab-warn-class-sources` (< 3 sources), `lab-warn-class-same-size` (aire max/min < 4 sur ≥ 10 boîtes), `lab-warn-class-same-place` (centres tous à < 0,1 de leur moyenne), `lab-warn-light-uniform` (écart de luminance moyenne < 50 sur ≥ 10 images ; luminance mesurée à l'import, colonne `luma`). Migrations `000017`, `000018`. | 2026-10-13 |
| **D206** | **Fichier de la médiathèque comme source de flux** (M0, §4.1) : nouveau type `media_asset` de `media_streams`, URL `media:<id d'asset>` (schéma `media`, aucun réseau, hors politique R8). À l'enregistrement : asset de l'org, version courante lisible par le décodeur (MP4/MOV, MP3, AAC, OGG, FLAC, WAV), jamais sur un boîtier de capture (`capture_on` refusé). À la capture : l'org est re-vérifiée, le fichier (≤ 512 Mo) est lu depuis le stockage, son `moov` remis en tête (D203), puis passé aux **mêmes décodeurs confinés** que les flux, avec `-re` : il est joué à sa **vitesse naturelle**, comme une source en direct (images sur le bus caméra au `fps` du flux, audio vers la transcription) ; horodatage = heure de relecture. Source **finie** comme `http_file` : le flux se désactive à la fin et sa fin n'est pas un silence. Le bouton Test d'un flux lit le fichier **sans** `-re` (extrait immédiat). | 2026-10-13 |
| **D207** | **M5 = Pages + nœuds génériques, pas de studio de rapports** (Shan, 2026-10-13). Un rapport est une **page** (pages.md) écrite par un flow et exportée en PDF ; la chaîne type est `inject` (cron) → données (`event_log`, `media_fuse`, métriques) → `llm_text` → `page_write` → `page_pdf` → `pnex_notify` (pièce jointe). Chaque nœud ne fait qu'une chose et sert ailleurs (compte rendu sans IA, PDF d'une procédure, classification LLM sans page). D173 est rouverte **étroitement** : un PDF de page peut sortir de PneX par un export humain ou par un flow déployé par un humain (logique D128), jamais d'envoi décidé par l'assistant. | 2026-10-13 |
| **D208** | **`llm_text`** : nœud générique, gabarit de consigne + données entrantes passées au LLM de l'org (D119) comme **données délimitées**, jamais comme instructions, aucun outil ; pas d'option « phrases sourcées » (des phrases sourcées s'obtiennent en amont avec `media_fuse`, dont la sortie peut nourrir `llm_text`) ; plafond quotidien d'appels IA commun (D196) ; sortie `{text, model, took_ms, data_truncated, ai_generated: true}` ; données ≤ 60 000 caractères (tronquées au-delà, signalé), consigne ≤ 4000, réponse 100–20 000 caractères. Route `/internal/flow/ai/text` (jeton + fencing). **Livré** le 2026-10-13. | 2026-10-13 |
| **D209** | **`page_write` et `page_pdf`** : `page_write` écrit du Markdown dans une page de l'org (nouvelle page depuis un titre à gabarit, ou remplacement d'une section nommée), par le convertisseur serveur existant (`services/pages/seed.rs`), puis crée une **version nommée** ; auteur « flow #id ». `page_pdf` rend une version de page en PDF (en-tête org + titre + version, pied « page N / M » et date, bandeau « généré par IA » si la version contient du texte IA) et le range dans la médiathèque (`kind=document`). Moteur PDF : **krilla** (MIT/Apache, ~24 crates, polices sous-ensemblées, mise en forme du texte) + mise en page PneX (titres, paragraphes, listes, tableaux, images) ; **Typst écarté** (128 crates dont `yaml-rust` non maintenu et un interpréteur WASM inutile). Police : **DejaVu Sans** (licence Bitstream Vera, permissive ; DejaVu au domaine public), notice jointe, pas d'OFL. Le même rendu sert le bouton « Exporter en PDF » de Pages (lot L7). **`page_write` livré** le 2026-10-13 : route `/internal/flow/page-write` (jeton + fencing, page cherchée dans l'org), ajout validé comme une édition (schéma, tailles) et relayé en direct aux éditeurs ouverts, convertisseur Markdown enrichi (gras, italique, code, liens sûrs, citations, filets, blocs de code, titres 1–6 ; tableau → paragraphes), versions `doc_versions.ai_generated / flow_id / approved_by / approved_at` (migration `000019`), `POST /api/v1/docs/{id}/versions/{n}/approve` (éditeurs, idempotent) et badges dans Pages. **`page_pdf` livré** le 2026-10-13 : crate `pnex-pdf` (krilla 0.8 sans ses features par défaut ; mise en forme du texte par **harfrust 0.14**, rustybuzz/ttf-parser étant signalés non maintenus — RUSTSEC-2026-0206/0192), A4 en deux passes (numérotation « Page n / m »), titres, paragraphes, listes, citations, filets, code ; **tableaux et images non rendus** (le schéma des pages n'a pas de tableau ; images = lot ultérieur). Bouton **PDF** par version dans Pages (`GET /api/v1/docs/{id}/versions/{n}/pdf?lang=fr|en`). Nœud `page_pdf` : source « version du message » ou « dernière version envoyable, une fois » (table `doc_version_exports`, migration `000020`, unicité par nœud) ; sortie 0 prêt / sortie 1 à approuver. **Pièce jointe de `pnex_notify` livrée** : champ `attach_field` (chemin de l'id média), seul SMTP joint le fichier (`multipart/mixed`), fichier illisible = envoi en échec ; le runtime lit le PDF par `/internal/flow/media/{id}` (jeton + fencing), qui ne sert **que** les exports `page_pdf` de l'org. | 2026-10-13 |
| **D210** | **Sortie d'un rapport IA : versions approuvées par défaut**. `page_pdf` et la pièce jointe de `pnex_notify` ont un mode : **« versions approuvées seulement »** (défaut : le flow crée la page et prévient un humain ; l'envoi n'a lieu qu'une fois la version approuvée dans Pages) ou **« automatique »** (envoi immédiat, bandeau « généré par IA, non relu » dans le PDF et le courriel). Raison : texte non fiable (transcriptions, légendes) → risque d'injection dans un document qui quitte PneX. **Livré** le 2026-10-13 : la porte est tenue par `page_pdf` (case « Approbation requise pour le texte IA », cochée par défaut) ; `pnex_notify` joint ce que `page_pdf` a laissé passer, il ne décide rien. Le PDF porte « relu et approuvé par X le … » ou « non relu ». | 2026-10-13 |
| **D211** | **Analyse d'un fichier de la médiathèque (proposé, à relire)** : bouton « Analyser » sur un asset audio / vidéo = flux `media_asset` (D206) à cadence `batch` : décodage sans `-re`, audio seulement, contre-pression (≤ 4 segments en attente), horloge fichier déterministe (`base` = date de la version média + `offset_ms`), file ASR derrière le direct et exemptée de `max_lag`, fin = port `finished` de `media_source`. Une analyse à la fois par org, ≤ 4 h. Détail et questions Q-A à Q-D : §4.1bis. | 2026-10-14 (proposé) |

### État des lots

- **M1** (2026-10-12, branche `feat/media-vision-studio`) : `vision_track` et
  `vision_zone` livrés (core + runtime + éditeur + NodeDoc + fiche KB
  `cameras.md` + i18n), tests unitaires (suivi, zones, validation) et test
  moteur réel (`pnex-flow-runtime/tests/vision_zone.rs`). Reste : essai sur
  caméra réelle (ESP32-CAM) et éventuel calque de zones sur les
  enregistrements de `/cameras`.
- **M3** (2026-10-13, même branche) : `vision_caption` et `media_fuse`
  livrés (D194–D196) : contrat `pnex-core::vision_ai`, routes internes
  `/internal/flow/ai/{caption,fuse}`, image dans l'abstraction fournisseur,
  `pnex_vision::prepare_jpeg`, formulaires, NodeDoc, fiche KB `cameras.md`.
  Tests : unitaires, moteur réel contre un backend simulé
  (`tests/vision_ai.rs`), routes (401 / 404 / 409). Reste : essai avec un
  vrai fournisseur (Claude ou VLM local) et une vraie caméra.
- **M3bis** (2026-10-13, même branche) : `text_decide` (D197) livré —
  crate `pnex-decide` (port de la mise en séquence + tract), test de
  parité `--ignored` contre la référence ONNX Runtime (anglais et
  multilingue), service backend, nœud, formulaire de questions, NodeDoc,
  fiche KB, `task laya:bundle`. Reste : POC calibration / fine-tuning sur
  ~200 fenêtres réelles (Q14), mesure sur Pi 5 (Q13).
- **M2a** (2026-10-13, même branche) : Vision Lab données (D198–D200) —
  migration `000014_vision_lab`, service + contrôleur `/api/v1/lab`, pages
  `/lab` et `/lab/:id`, fiche KB `lab.md`, 11 codes d'erreur. Tests :
  backend (cycle complet + isolation org + lecteur 403), e2e `lab` et
  `vision-studio` (en + fr) contre un serveur de la branche. **M2b
  (entraînement) reporté**, voir D200. Pré-annotation (D201) vérifiée
  avec YOLOX-nano réel sur `dog.jpg` (chien, vélo, voiture reconnus et
  mappés, « chat 38 % » laissé en proposition grise). Import de trames
  d'enregistrement (D202) testé (segments AVI réels, trou d'enregistrement,
  bornes). Vidéos + interpolation (D203) : test réel du décodeur confiné sur un MP4
  H.264 au `moov` final (crate `pnex-media-capture`, `tests/video_file.rs`)
  et parcours backend complet (import, refus d'une image, interpolation
  avant / après annotation, image fixe refusée). Reste : vérification
  navigateur des deux nouveaux boutons et propagation par suivi (M4).
- **M2b** (2026-10-13, même branche) : entraînement sans Python (D204).
  Tests : `pnex-vision` (apprentissage réel d'une classe synthétique sur
  YOLOX-nano, mAP@0,5 = 1,0 en validation, 3,9 s en release), backend
  (parcours complet jusqu'au modèle enregistré, 409 si un entraînement est
  déjà en cours, lecteur 403). Reste : vérification navigateur du panneau,
  essai sur un vrai jeu de données.
- **M4** (2026-10-13, même branche) : D205 — file « à revoir »,
  propagation par suivi, guidage de diversité. Tests : cœur (bande de
  doute, propagation, avertissements), backend (file via la route interne
  avec Valkey réel : 401 sans jeton, 404 clé d'une autre org, classe
  reprise, sortie de file à l'annotation ; propagation sans image
  précédente → 422 ; lecteur 403). Reste : vérification navigateur.
- **INT8** (évalué le 2026-10-13, **non livré**) : tract exécute les
  opérateurs quantifiés ONNX (`QuantizeLinear`, `DequantizeLinear`,
  `QLinearConv`, `ConvInteger`). Une quantification des **poids seuls**
  réduirait le fichier par 4 mais tract la replie en float32 à
  l'optimisation (aucun gain de vitesse) ; un vrai gain demande une
  quantification **statique** avec calibration des activations
  (`QLinearConv`), projet à part, à ne lancer qu'après la mesure sur Pi 5
  (Q13) : YOLOX-nano à 1 image/s tient déjà sur CPU.
- **M0 terminé** (2026-10-13) : D206 — fichier de la médiathèque comme
  source de flux. Test backend avec un vrai MP4 (H.264 + AAC) : refus
  d'une image et d'un boîtier, extrait de test décodé.

