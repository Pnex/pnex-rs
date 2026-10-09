# Bibliothèque média (D21)

> Base d'assets média versionnée par org — **couche 1** de la vision média :
> 1. **Bibliothèque versionnée** (ce document) — photos, panoramas 360°,
>    splats/photogrammétrie, upload + previews + versioning nettoyable ;
> 2. Constructeur de visite 3D avec map (les médias sont **référencés**,
>    jamais dupliqués) ;
> 3. Player de tour + annotations (le domaine `sites`/`svg_files`/
>    `annotations` déjà schématisé, sans contrôleur, les accueillera).

## Modèle de données

Migration `m20260909_000012_media` (PK UUID, école sites) :

- **`media_assets`** — identité : `org_id` (D2, CASCADE), `kind`
  (`photo|panorama|splat`, string), `name`, `description`, `metadata` JSONB
  (tags libres ; futur zone/étage), `current_version_id` (FK circulaire
  PG-only SET NULL — école `flows.deployed_version_id` ; intégrité portée
  par le contrôleur sur sqlite).
- **`media_versions`** — append-only : `asset_id` CASCADE, `org_id`
  **dénormalisé** (scoping direct + purge fiable), `version_number`
  (incrémental par asset, unique `(asset_id, version_number)`),
  `filename` (sanitisé), `content_type`, `size_bytes`, `storage_key`,
  `sha256`, `metadata` JSONB (**GPano/EXIF sniffés — par version**),
  `note`.

## MediaStore — octets jamais en base

`services/media.rs` : trait `MediaStore {put,get,delete,exists}` (école
`ArtifactStore` mais trait local), clés `org_{org}/media/{asset}/{version}_{filename}`
(sanitize `pnex_firmware_builder::sanitize_segment`), purge par clés connues
en DB, idempotente. Sélecteur `MediaSettings::store()` (école
`FirmwareSettings::store`) :

- **`fs`** (défaut) — opendal `services::Fs`, racine `settings.media.dir`
  (env `PNEX_MEDIA_DIR`, défaut `./storage/media`) — dev, tier hobbyist ;
- **`s3`** — opendal `services::S3`, réutilise les vars `PNEX_S3_*` (RustFS,
  tier industriel).

Écriture versionnée : **storage d'abord, DB ensuite** (un blob orphelin est
purgeable, une ligne sans blob ne l'est pas) ; échec DB → blob fraîchement
écrit supprimé best-effort.

## Surface API

Contrat : `docs/contracts/media.http`. Endpoints **additifs** — ne jamais
bumper `pnex_api_contract::CONTRACT` (un bump bloquerait la Gate sur les
vieux clients).

- `POST /api/v1/media?name=&filename=&kind=&content_type=` — upload
  **raw bytes octet-stream** (premier chemin body-bytes du repo ; pas de
  multipart — reqwest front sans feature, marche identique wasm + Android).
  `DefaultBodyLimit` posé par handler (env `PNEX_MEDIA_MAX_BYTES` défaut
  256 Mo + 1 Mo de marge ; le 413 JSON propre part du handler via
  `content_length()`).
- `GET /api/v1/media` (D14 : `kind`, `search`, pagination envelope),
  `GET/PATCH/DELETE /{id}`, versions : `GET/POST /{id}/versions`,
  `GET/DELETE /{id}/versions/{n}`, `POST /{id}/versions/{n}/restore`,
  octets : `GET /{id}/content`, `GET /{id}/versions/{n}/content` (inline).
- Versioning append-only nettoyable : POST = n+1 et devient courante ;
  DELETE version refuse la dernière (`409 {"error":"last_version"}`) et
  re-positionne la courante sinon ; restore re-positionne ; DELETE asset
  purge tous les blobs.

## Sniff GPano (kind auto-détecté)

`services/media_sniff.rs` (pur, testable) — priorité : kind client > sniff >
`photo` :

- `GPano:` + `equirectangular` dans les 128 premiers Ko (XMP APP1 du JPEG)
  → `panorama` — même détection que Google Street View, **zéro stitching à
  implémenter** : les photo spheres GCam/Insta360 importées sont reconnues
  d'office ;
- extensions `.ply|.splat|.ksplat|.spz` → `splat` ;
- tout le reste → `photo` (cubemaps/tuiles stockables via kind forcé).

## Viewers web (pattern tron-gerbe)

- `js/viewers.js` → esbuild IIFE `assets/viewers.js` + `assets/viewers.css`
  (gitignorés + stubs `js:ensure` — la macro `asset!()` exige les fichiers à
  la compilation) : pannellum 2.5.7 (WebGL1, équirect) + gsplat 1.2.x
  (WebGL2, `.splat`/`.ply` via `SceneFormat.fromArrayBuffer`).
- Globale `window.pnexViewers = {panorama:{mount,unmount}, splat:{...}}` ;
  pont Rust `src/media_viewer.rs` (wasm `js_sys::Reflect`, natif
  `document::eval` — prouvé par tron.rs) ; échec → `false` → badge
  « aperçu indisponible », jamais de panique UI.
- **Piège npm** (2026-09-10) : `require('pannellum')` renvoie un objet
  **vide** — le paquet est un « global script » qui pose `window.pannellum`
  en effet de bord (idem classe de piège pour l'ESM pur de gsplat).
  `js/viewers.js` valide la forme (`typeof lib.viewer === 'function'`,
  `typeof lib.Renderer === 'function'`) et retombe sur la globale.
- **Piège CSS** (2026-09-10) : `pannellum.viewer(host)` pose la classe
  `.pnlm-container` (**height: 100%**, viewers.css chargé après Tailwind)
  **sur le div hôte lui-même** → écrase `h-[420px]` ; `height:100%` d'un
  parent flex non dimensionné = **0 px** → canvas invisible (boîte noire,
  viewer pourtant `loaded`). Taille du host en **style inline**.
- Blob URLs obligatoires : les `<img>` et viewers ne posent pas d'en-têtes →
  `util::media_blob_url` fetch en Rust (Bearer/X-Org-Id/refresh gérés) puis
  `URL.createObjectURL` (web-sys features `Blob`/`Url`). **Les octets
  transitent en mémoire du webview** (V1, acceptable aux plafonds choisis).
- **Progression + cache navigateur** (2026-10-08) : le téléchargement est
  streamé (`client::request_bytes_conditional`) et affiche
  `MediaDownloadProgress` (Mo reçus / total, %) sur les viewers splat/pano
  (bibliothèque, Annotations, Visualisation, aperçu POI) ; gsplat montre un
  spinner pendant le parsing jusqu'à la 1re frame. Les octets sont gardés en
  **Cache Storage** (`media_cache.rs`, web seulement, budget 1,5 Go, plus
  ancien évincé d'abord, purgé au logout) sous l'`ETag` = id de version
  (immuable). Chaque vue **revalide** (`If-None-Match`) : le serveur re-scope
  l'org avant de répondre 304 sans corps — aucune lecture sans autorisation,
  nouvelle version vue immédiatement (`Cache-Control: private, no-cache`).
  Pas le cache HTTP : les navigateurs y refusent les entrées de plusieurs
  dizaines de Mo (justement les splats).
- **Limites V1** : `.ksplat`/`.spz` stockables sans aperçu (**Spark.js** couvre
  les 4 formats — plan B documenté) ; pas de vignettes en liste (fetch complet
  par carte) ; previews **web-only** en natif (le fetch JS cross-origin de la
  webview serait bloqué CORS — étape 2 : serving média same-origin ou proxy).

## Capture photo Android

`src/capture.rs` (JNI brut, école patch webbrowser vendu — tables `v1_1`/
`v1_2`, jamais de macros haut-niveau, `::jni::sys` OBLIGATOIRE) :

- **Aucune permission CAMERA déclarée** → aucune mécanique runtime-permissions
  (l'intent caméra système détient la sienne) ;
- temp dans `getCacheDir()/camera/`, `FileProvider.getUriForFile`
  (autorité `{package}.fileprovider` — jamais hardcodée, ref global JNI),
  `Intent ACTION_IMAGE_CAPTURE` + `EXTRA_OUTPUT` + flags 0x40|0x80 ;
- manifest/gradle : `patch-android-manifest.py` ajoute le `<provider>`,
  (re)crée `res/xml/file_paths.xml` (cache-path `camera/`) et la dep
  `androidx.core` — idempotent, regex tolérantes (le projet gradle est
  régénéré à chaque `dx build`) ;
- résultat **jamais par `onActivityResult`** (MainActivity générée par dx) :
  `spawn_forever` + polling 500 ms (timeout 10 min) — taille stabilisée →
  upload version 1 → purge du temporaire dans tous les cas ;
- freezer arrière-plan API 36 : le polling ne tourne qu'au premier plan —
  suffisant (le retour caméra remet l'app au premier plan) ; exception
  fantôme post-`startActivity` : classée (clear + poursuite).

## Étude — panorama 360 capturé depuis le mobile

- **Option A — import GPano** : l'app n'implémente aucun
  stitching ; les panoramas viennent de l'app caméra système (photo sphere
  GCam) ou Insta360, l'import sniff le XMP GPano. Qualité pro, zéro R&D.
- **Option B (réalisée, Take 360 v1) — capture guidée in-app** : overlay
  « tournez sur place », 16 frames + stitching **on-device pure Rust**.
  Cf. section suivante.
- **Option C — hardware 360** (Insta360/Ricoh via SDK) : import simple,
  dépend d'un hardware dédié.

## Take 360 v1 — capture panoramique guidée on-device (2026-09-09)

Chemin « quick and dirty » : bouton **Take 360** (Android, owner/admin)
à côté de « Prendre une photo ». Les pros importent leurs vrais 360 (option
A) ; Take 360 couvre ceux qui n'ont rien sous la main.

### Chaîne

```
[overlay dioxus] preview getUserMedia (webview, secure context
      │          WebViewAssetLoader) + SVG guidage (point guideur,
      │          cercle, dwell ring) — tick 33 ms lit des atomics
      │
[capteur] thread std + FFI ndk-sys (TYPE_ROTATION_VECTOR, 30 Hz) →
      │     yaw/pitch/roll convention pnex-stitcher, millidegrés
      │
[frames] canvas 1000 px → JPEG q80 → base64 chunké 128 KiB (eval dioxus)
      │
[pipeline] thread OS stitch (2048 px, blend plateau, GPano XMP) →
      │       upload POST /api/v1/media kind=panorama (contrat inchangé)
      ▼
sniff backend classe panorama ; pannellum rend (aucune modif viewers)
```

### Choix structurants

- **Stitching pure Rust sans OpenCV** : la crate `crates/pnex-stitcher`
  (workspace) place les frames par la **pose capteur** uniquement — pas de
  détection de features (dérive magnétomètre acceptée en v1 ; raffinement
  phase-correlation = v2). Couverture mesurée **dans la bande couverte**
  (un anneau horizontal ne couvre qu'une tranche de l'équirect), seuil
  0,98 — une frame manquante (~11 %) est refusée, les bords partiels
  (~1 %) passent. Pôles étalés + trous intra-bande comblés (Q&D).
- **Permission CAMERA** : déclarée au manifest par
  `patch-android-manifest.py` (step idempotent `patch_camera_permission`).
  La demande runtime est déjà câblée dans **wry**
  (`RustWebChromeClient.onPermissionRequest` → launcher Capacitor) pour
  getUserMedia ; côté intent photo, pre-check dans `capture.rs`
  (déclarée + non accordée = SecurityException sur ACTION_IMAGE_CAPTURE).
- **Capteur via FFI ndk-sys** : la crate `ndk` 0.9 n'a pas de module
  sensor — `ASensorManager`/`ALooper` bruts sur un thread dédié, zéro
  classe Java. Signes référentiels isolés (ROTATION_DIR, matrice caméra
  arrière S·Rᵀ) — **calibration smoke test device obligatoire**.
- **Modèle de guidage** (concept réimplémenté, pas copié — les deux repos
  de référence ont des licences incompatibles/inadaptées) : 16 pas de
  22,5°, tolérances 2°/1°/4°, dwell 1 s, point guideur ~1 px/° — école
  flutter-plugin-camera360 (Apache-2.0, Dart/C++) et Panorama (Rust,
  ANTI-CAPITALIST : concepts de warp/blend/calibration seulement).
- **Écoles suivies** : `capture.rs` (atomics + polling + spawn_forever +
  stubs hors Android), `media_viewer.rs` (JS via `document::eval`).

## Take 360 V2 — stitching qualité serveur (2026-09-10)

### Aligonnement par modèle IA (2026-09-10, feature `pose-model`)

L'alignement NCC translation-only ne corrigeait pas les erreurs de
ROTATION des poses capteur (magnétomètre intérieur, roll gelé) — capture
réelle 2 anneaux : ghosting géant (~100 px). Ajout d'un chemin **rotation
par les images** : SuperPoint+LightGlue en ONNX (feature cargo
`pose-model`, server-only — le device reste pure Rust sans natif), via le
crate `ort` (ONNX Runtime) :

1. `pose_model.rs` : extraction par frame (gris ≤ 896 px, multiple de 8)
   puis matching LightGlue par paire — **keypoints normalisés [0,1]**
   (l'export v1 « fused_cpu » attend la normalisation : 567 matches vs 44
   en pixels bruts, mesuré A/B) ;
2. `rotation.rs` : rotation relative par RANSAC (Horn quaternion,
   itération de puissance ; `atan2(|a×b|, a·b)` pour l'erreur — acos y
   perd toute précision f32 sous ~1°) ;
3. `align::solve_rotation_gn` : Gauss-Newton 3 DOF par frame (yaw/pitch/
   roll — le roll n'est plus gelé), résidus géodésiques log(predᵀ·rel)
   pondérés √inliers×√Huber(0,5°) — pondérer par h (et non √h) rend le
   coût CONSTANT dans la queue du Huber → gradient nul, solve immobile
   (bug corrigé, constat solve synthétique) ; région de confiance (pas
   plafonné à 1°) contre l'overshoot ; prior capteur par axe (yaw faible
   0.3, pitch/roll 1.5 — le magnétomètre intérieur est la principale
   source d'erreur, la gravité est fiable).
4. Repli NCC si paires acceptées < n/3 (flag `AlignReport::fallback`,
   journalisé). Captures : 21 paires acceptées, correction max 9,7°,
   ghosting éliminé.

Modèles v1 de fabio-sim/LightGlue-ONNX **committés** dans `deploy/models/`
(superpoint.onnx 5,3 Mo + superpoint_lightglue_fused_cpu.onnx 45,6 Mo —
formes dynamiques : 1 extraction par frame, le transformer ne tourne que
par paire ; les exports « pipeline » v2/v3 refont l'extraction aux deux
images de chaque paire). Répertoire : `PNEX_STITCH_MODELS_DIR`.
Concurrence : `PNEX_STITCH_MAX_CONCURRENT` (défaut 2, sémaphore tokio —
décision 2026-09-10 pour RPi 8 Go, < 10 min/job).

### Pipeline Quality (pnex-stitcher)

L'aperçu device reste en V1 (sélection + pôles habillés, 1024) ; les
frames + poses repartent au serveur qui assemble en **pipeline Quality**
(pnex-stitcher `Pipeline::Quality`) et attache le résultat comme
**nouvelle version** de l'asset (modèle versions média).

### Pipeline Quality (pnex-stitcher)

Trois étages issus de l'étude des références (OpenCV/OpenStitching,
OpenPano, stitchEm — cf. commit de43b3b puis 2026-09-10) :

1. **Alignement par les images** (`align.rs`) : strips équirect gris
   blanchis (high-pass + érosion du masque — les halos de bord biaisent
   le pic NCC de ~1 px), recherche translation NCC par paire (fenêtre
   ±4°, fit parabolique, dalles fusionnées), solve Gauss-Newton avec
   prior Tikhonov (la jauge), IRLS/Huber (down-weighting des pics
   parasites) et **rollback au meilleur état** (résidu total pondéré
   minimal). Conventions verrouillées par sonde unitaire (`m = Δ_a − Δ_b`,
   résidu `(δb−δa) − m` → δ = −Δ) ; invariant zéro-jitter → zéro
   correction testé. Précision synthétique ~1,5° pour un jitter ±1,5° ;
   rejets NCC → dégradation propre vers les poses capteur.
2. **Gains per-channel** (`render::estimate_gains(channels)`) : R/G/B
   indépendants. Correctif important : la relaxation était **Jacobi** et
   oscillait sans converger sur les graphes bipartis (anneau pair) —
   gains restés neutres en V1 ; Gauss-Seidel en place (constat 2026-09-10).
3. **Coutures DP** (`seam.rs`) : plus court chemin par ligne (|Δs| ≤ 1,
   wrap circulaire, demi-résolution) sur le coût |Δcouleur gainée| →
   `OwnerMap` binaire par pixel. Testé en routant un obstacle (occlusion).
4. **Multi-bandes** (`multiband.rs`) : pyramides de Laplace, mécanique
   OpenCV exacte (numérateur = Σ lap(img⊙mask), dénominateur = Σ
   gauss(mask), normalisation par niveau + collapse). Poids = masque
   binaire **seul**. Wrap circulaire horizontal. Correctif noyau expand
   pair×pair (rangées cj−1 **et** cj). Fills **après** le blend.
   DÉFAUT **2 niveaux** (A/B capture réelle 2026-09-10 : adoucit
   visiblement les coutures sans voile ; ≥4 niveaux = halos + destruction
   du remplissage des trous — maille grossière > bandes de propriété).
5. **Remplissages** (`render::fill_band_holes` / `fill_poles`) : trous
   intra-bande par **extension verticale** (fusion des voisins couverts
   au-dessus/dessous, poids 1/distance, puis lissage horizontal rayon 2
   des pixels remplis) — l'ancien remplissage horizontal étirait les bords
   en traînées « échelle » sur les trous larges. Calottes polaires :
   dégradé bord lissé → teinte moyenne (inchangé).

### API stitch-jobs (additive, CONTRACT inchangé)

- `POST /api/v1/stitch-jobs` {asset_id, frames_total, hfov_deg, poses[]} —
  crée le job (`state=queued`) ;
- `POST /api/v1/stitch-jobs/{id}/frames/{k}` — octet-stream JPEG ;
- `GET /api/v1/stitch-jobs/{id}` — état (poll UI, modèle « build_phase »).
- `GET /api/v1/stitch-jobs/by-asset/{asset_id}` — dernier job d'un asset
  (404 si aucun) : source de vérité de l'overlay UI « version HD en
  préparation » posé sur l'aperçu device tant que le job est
  `queued|running` (poll 4 s côté page médias ; à `succeeded` la page
  toast + recharge le détail — le preview se remonte sur la nouvelle
  version courante).
- Worker `StitchPanoramaWorker` (queue pg_loco, pattern BuildFirmware) :
  décode, stitch Quality à `settings.stitch.out_width` (4096, env
  `PNEX_STITCH_OUT_WIDTH`), `write_version` sur l'asset (v2 = HD), state
  → succeeded/failed. Frames conservées sous `stitch_jobs/{job_id}/`
  (re-stitch/tuning hôte).

### Tuning sur capture réelle (2026-09-10, job a5dc3476)

Boucle hôte `restitch` + `audit_poses` (examples, feature `pose-model`) :
rendu → lecture visuelle par zones → knob → re-rendu. Constats :

- **Captures incomplètes** : anneaux non refermés (équateur 15 frames ×
  ~20,3° + hfov 43,7 ≈ 327° < 360° → **trou azimutal ~33°** ; anneau haut
  ~344°) — le remplissage vertical fait la meilleure image possible, pas
  de l'inventé. Le guidage capture doit exiger la **fermeture de l'anneau**
  (retour au yaw de départ ± recouvrement) avant de valider un anneau.
- **Anneau haut (pitch +47°) inalignable** : plafond blanc sans texture →
  SuperPoint/LightGlue 0 paire (audit_poses) → placement = poses capteur.
  Toute erreur magnétomètre devient une erreur de rendu plafond. Murs
  blancs = même limite pour l'ancrage inter-anneaux.
- Priore yaw modèle (λ 0,3→10), seam full-res, NCC inter : aucun gain
  visible supplémentaire. Défauts résiduels (zigzag mur/plafond ~1°,
  lumière du plafond décalées de ±2°) = bruit capteur sur contenu sans
  texture — réduction par re-capture (trépied) ou alignement structure
  (lignes), pas par knobs.

### Take 360 V3 — guidage pas-à-pas strict (2026-09-10)

Remplace le déclenchement continu (V1) qui tirait **en marche arrière**
(|Δyaw| comptait le retour utilisateur : « photo déjà prise » pendant une
correction, « le suivant a pris du retard ») et sous-capture l'anneau
(trou ~33°). Modèle des apps 360 du commerce :

- **Plan d'anneau calculé** : hfov capteur (Camera2, paysage) × w/h du
  track = hfov portrait → `guidance::ring_plan` : pas = hfov − 20°,
  N = ceil(360/pas) ∈ [10, 20] — recouvrement ≥ 20° et fermeture
  d'anneau PAR CONSTRUCTION ((N−1)·pas + hfov ≥ 360). À 43,7° →
  16 cibles de 22,5°. Le plan est posé à l'ouverture de session
  (track size connu après open_camera).
- **Déclenchement gated** : une frame ne part QUE sur la cible courante
  (yaw = k·pas relatif à l'ancre, pitch = consigne d'anneau ±40° bande
  ±12°), quand alignement (tolérances 6/8/10° — les 2/1/4° de la
  machine en réserve étaient intenable à la main) + stabilité (vitesse
  angulaire max 3,5°/s sur fenêtre 450 ms) + dwell 700 ms tenus EN
  CONTINU. Tourner en arrière, rater l'inclinaison, bouger : RIEN ne
  part. La pose est relative à l'ancre utilisateur (« Ancrer ici ») —
  les cibles k·pas sont relatives, reancrage à tout moment.
- **UI** : point guideur (précède, `dot_offset_px`), chevron directionnel
  + degrés, arc de dwell autour du réticule, pastille stabilité
  (verte/orange), flash de capture, bannières discrètes (tilt/turn/
  hold/keep), compteur « photo k/N », barre anneau + % total. Le
  CONTINU (dot/arc/flèche/degrés/pitch/flash/stabilité) est piloté en
  DOM direct à 15 Hz — le re-render dioxus reste réservé aux changements
  discrets (GPU webview saturé au-delà, MALI BAD ALLOC constaté).
- **« Reprendre »** : retire la dernière frame et recule la cible
  (`UNDO_REQUEST` traité par la boucle driver, single-writer de
  frames_mem) — pas d'undo inter-anneaux (cible 0 du nouvel anneau =
  seule option ; recapture de l'anneau entier si besoin).
- Tests hôte (guidance.rs) : fermeture d'anneau pour toute hfov ∈ [30°,
  75°], plan 43,7° → 16×22,5°, transitions decide (dwell annulé par
  instabilité/dérive/pose perdue), advance sans saut, vitesse angulaire
  (wrap 179→−179 = 20°/s, pas 358°).

### Test device V3 — conventions posées (2026-09-11)

Premier test device du pas-à-pas : deux conventions inversées trouvées et
corrigées le jour même.

- **Miroir gauche↔droite du panoramique** : le mapping interne du stitcher
  (u croît avec la longitude ENU) rend une image en miroir à l'affichage
  « depuis l'intérieur » (standard : u doit croître en tournant à DROITE).
  Auto-cohérent — coutures propres, align OK — mais inversé vs. la scène.
  Fix **à la frontière** : `mirror_rows` + `mirror_mask` en fin de
  `stitch()` (rgb + masque ; `covered` est invariant) — le pipeline
  interne (align NCC, solve, seams, multiband, gains, fills) garde la
  convention historique verrouillée par les sondes. Sonde `rendu_handedness`
  (lib.rs) : frame split blanc/noir à yaw 30° → le blanc doit sortir à
  GAUCHE du centre (texte lisible = non-miroir). Validé en réelle :
  re-stitch hôte du job `060dd4ef` (48 frames 3 anneaux, 81 s,
  couverture 0,96, align convergé, correction max 13°) — texte des écrans
  lisible ; version asset republiée in-place (sha + taille resync DB).
- **ROTATION_DIR = −1** (était +1) : la pose yaw est l'azimut ENU
  (croît en tournant à GAUCHE) — avec +1, chevron/point guideur pointaient
  le mauvais côté → poursuite sans convergence (« impossible de centrer »,
  point « en dessous » hors cercle au drift de posture). Le test device
  liste explicitement ce point comme non validé — il l'est maintenant par
  le raisonnement ENU + le retour utilisateur du matin. Pré-ancre : le
  point guideur reste au CENTRE et chevron/degrés masqués (la cible
  pré-ancre est relative au zéro de session, souvent capté en mouvement —
  instruction = « visez + Ancrer ici »).
- Le **pitch est bien l'élévation** du regard (look up → pitch positif) :
  la chaîne euler n'a pas bougé.

### Auto-calibration de focale (2026-09-11, après-midi)

Le HD 3 anneaux restait « pourri » (ghosting + franges chromatiques) après
le fix miroir. Root cause : **le hfov du track n'est pas le hfov de la
caméra** — il dépend du mode capteur négocié par le webview pour la
résolution demandée : même ratio w/h, 720p → ~52° hier, 1080p → ~40°
aujourd'hui (43,7° déclaré). Le biais gonfle les rotations mesurées par
features de +12 %, l'aligneur ne peut pas satisfaire toutes les paires à
la fois → ghosting ; les gains locaux/per-channel amplifient ensuite en
franges roses/cyan.

- **Diagnostic par `audit_poses`** : les paires adjacentes mesurent ~28°
  pour des pas capteur de 22,5° — inversion du modèle tan → focale réelle.
  À hfov 39-40° : biais résiduel ~0, corrections max 12,5° → 8,7-9,4°,
  rejets 94 → 72, rendu net (écran lisible, franges disparues).
- **Fix structurel dans `refine_poses_model`** : les correspondances
  LightGlue sont en pixels (focale-indépendantes) → recherche sur grille
  de l'échelle de f_px qui annule le biais médian (angle de rotation
  mesuré − angle capteur) sur les paires fortes (≥ 30 inliers), puis
  pipeline à la focale calibrée. < 5 paires fortes → focale inchangée.
  Validé réel : ×1,100 → hfov 40,1° (biais +0,08°) retrouvés depuis le
  43,7° déclaré, rendu = rendu manuel-39. Journalisé dans
  `AlignReport::calibrated_hfov`.
- **`GAINCH=1`** (gains luminance) supprime aussi les franges à focale
  fausse — garde A/B, pas passé en défaut (la calibration traite la
  cause).
- **Warp local (mesh) : DÉSACTIVÉ par défaut (2026-09-11 soir)** —
  l'implémentation existe (`align::estimate_local_warp`, grille 7,5°,
  mesure NCC sub-pixel, jauge δa=−m/2/δb=+m/2 verrouillée par la sonde
  `sonde_warp_local`, application via `project::sample_ray`) mais la
  grille libre interpolée depuis des mesures isolées crée des
  distorsions haute fréquence qui AMPLIFIENT le contraste local au rendu
  4096 (« hyper contrasté, cassé » — constat utilisateur, l'A/B à 2048
  sur crops étroits ne le montrait pas). **Leçon de validation : tout
  défaut qualité se juge en PLEINE FRAME à la résolution de
  publication.** Réactivation : refit polynomial/régularisé (modèle
  lisse par construction) + validation 4096 pleine frame. Opt-in :
  LOCALWARP=1.
- **Deux bugs du multibande corrigés (2026-09-11 nuit, défaut LEVELS 3)**
  : les halos blancs saturés, franges orange/cyan et diffusion en
  fantômes du « blending » venaient de DEUX bugs de la pyramide
  (diagnostic par portage Python isolé de la pyramide) :
  (1) **signe du Laplacien inversé** — num accumulait expand(G) − G =
  −L : le collapse SOUSTRAIT le détail (reconstruction fausse dès qu'il
  y a de la HF ; le test reconstruction_identité ne le voyait pas, son
  sinus doux passant sous la tolérance → octave fine ajoutée) ;
  (2) **numérateur non pondéré par le masque** — chaque frame déversait
  son contenu PLEIN dans le recouvrement (deux frames constantes 100/200
  en recouvrement total sortaient ~300, clampé 255 = halos blancs).
  Fix : L = G − expand(G), tout pondéré par la Gaussienne du masque.
  Tests : `recouvrement_constant_sans_double_comptage` (100 → 150 → 200,
  jamais ≥ 250) + reconstruction à octave fine. Défaut multiband_levels
  2 → 1 → **3** (fondu large sans fantôme). LEVELS=1 = chemin plat
  crossfade + fondu étroit de couture (SEAM_FEATHER_PX, moyenne pondérée
  pure — la pyramide AMPLIFIE une rampe étroite) + seuil covered 0,5 →
  0,05 (masques fondus).
- **Warp local réactivé par défaut (2026-09-11 nuit)** : avec la
  pyramide corrigée, le warp (95 tuiles, 43 champs, franges cuivre sur
  les edges bois et objets déchirés éliminés) devient net positif — le
  « hyper contrasté » du soir venait du double-comptage du multibande
  qui amplifiait ses corrections, pas du warp lui-même. Validation
  pleine frame warp × pyramide corrigée sur les 3 captures. Knob
  LOCALWARP=0 pour A/B.
- **Contenu DOUBLÉ en zone sans texture — corrigé (2026-09-11 soir)** :
  les paires de la zone blanche (tableau, mur, plafond) étaient rejetées
  au seuil `model_min_inliers = 15` → aucune contrainte d'image → poses
  capteur brutes → contenu doublé de ~30 px à 4096 (constat utilisateur
  « cassé de partout », capture 52fdd79b zone tableau). **Défaut 15 → 8**
  : +21 paires acceptées (73 → 94), tableau et poutres redeviennent
  simples ; Horn/RANSAC reste gardé par `model_max_err_px`. Suites de
  tests vertes. Le résidu de blend (fuite basse-fréquence du multibande
  sur objets sombres) reste un chantier séparé.

### Suite (hors scope de cette passe)

- Capture **portrait 2 anneaux** (16 à plat + 12 incliné pitch ~65°) :
  vfov portrait ≈ 65° → 2 anneaux suffisent, pôle nord réel (3 anneaux
  paysage sinon). Chantier capture-protocol, UX overlay à refondre.
- Montée de résolution de capture (720 → 1080+ px, sans downscale).
- iOS, capture web, shutter manuel, calibration par device.
