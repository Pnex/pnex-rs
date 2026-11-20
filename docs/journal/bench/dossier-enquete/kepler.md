# Fiche benchmark : kepler.gl (Trip layer, animation, filtre temporel)

## 1. Identité

- **Dépôt** : `keplergl/kepler.gl`, clone local `bench/kepler.gl`.
- **Commit étudié** : `c9f6ecb791a7295bd2d2963d3f602b20516c7204` (12/10/2026).
- **Licence** : **MIT**, confirmée.
  - `LICENSE:1-3` : « Copyright contributors to the kepler.gl project / Permission is hereby granted, free of charge… » (texte MIT).
  - `package.json:6` et `src/layers/package.json:6` : `"license": "MIT"`.
  - Chaque fichier lu porte l'en-tête `// SPDX-License-Identifier: MIT` + `// Copyright contributors to the kepler.gl project` (vérifié par `grep -L` : aucun fichier sans en-tête dans `src/layers/src/trip-layer/`, `src/components/src/common/animation-control/`, `src/reducers/src/vis-state-updaters.ts`).
  - Seul autre fichier de licence : `src/sqlrooms/LICENSE` = MIT aussi (« Copyright 2024-2026 SQLRooms Contributors, Copyright Vis.gl contributors »). Pas d'édition entreprise, pas de sous-dossier sous une autre licence.
- **Dépendance clé** : deck.gl `^9.3.1` (`src/layers/package.json:33-35`), `TripsLayer` vient de `@deck.gl/geo-layers`. deck.gl est **MIT** (Copyright vis.gl contributors).
- **Ce que la licence permet à PNEX (MIT)** : on peut **reprendre du code** (algorithmes, shaders GLSL, logique des fenêtres d'animation) avec attribution : garder l'avis de copyright MIT dans le fichier repris, ou dans un `THIRD_PARTY_NOTICES`. Compatible avec la règle « briques permissives obligatoires » de PNEX. En pratique on **transpose** surtout en Rust : seuls de petits morceaux (pseudo-code d'interpolation, constantes) passent tels quels.

## 2. Ce que fait le projet

kepler.gl est une application React/Redux de géovisualisation de gros jeux de données, rendue par deck.gl (WebGL) au-dessus d'un fond Mapbox/MapLibre. Le **Trip layer** anime des trajets : une traînée qui s'estompe derrière une tête mobile, pilotée par une horloge globale (`animationConfig`). Le **filtre temporel** (`timeRange`) affiche un histogramme en bas de carte avec une fenêtre glissante qu'on peut lire comme une vidéo.
**Ce qui nous concerne** : le format `[lon, lat, alt, ts]` (repris tel quel par le PRD), la conversion du temps, le calcul de la position interpolée au curseur, les deux horloges (point ou fenêtre) et leur synchronisation, l'ergonomie de la frise (molette, pincement, Ctrl+flèches), la sérialisation versionnée. kepler ne fait **rien** sur le graphe, les rôles ni la provenance.

## 3. Modèle de données pertinent

### 3.1 Trip layer : entrée et colonnes

- Deux modes de colonnes, `src/layers/src/trip-layer/trip-layer.ts:281-296` :
  - `geojson` : une colonne GeoJSON dont les features sont des `LineString` à coordonnées `[lon, lat, alt, ts]` ;
  - `table` : colonnes `id, lat, lng, timestamp` (+ `altitude` optionnelle). Les points sont regroupés par `id` en LineString, dans `groupColumnsAsGeoJson` (`src/layers/src/geojson-layer/geojson-utils.ts:494-554`).
- Le mode table **trie** les points par timestamp (`geojson-utils.ts:542-544`, `items.sort(ascending(a[3], b[3]))`), sur la **valeur brute** de la colonne. Chaque coordonnée garde un pointeur sur sa ligne source (`coords.datumIndex`, `coords.datum`, `:519-521`), ce qui permet le tooltip et l'interpolation d'attributs (roll/pitch/yaw).
- Le mode GeoJSON, lui, **ne trie pas** : l'ordre du fichier fait foi.
- Config de calque, `trip-layer.ts:63-118` : `TripLayerVisConfig` = `opacity, thickness, colorRange, trailLength, sizeRange, billboard, fadeTrail, sizeScale`, plus l'orientation 3D (`adjustRoll/Pitch/Yaw`, `invert*`, `fixed*`) et un modèle 3D (`scenegraph*`). Champs `rollField/pitchField/yawField` pour orienter un modèle glTF.
- Valeurs par défaut :
  - `trailLength` : 180, plage [1, 1000], **en secondes** (`src/constants/src/layers.ts:122-132`) ;
  - `fadeTrail` : true (`:133-139`) ;
  - `thickness` : 0.5 (`trip-layer.ts:124`).
- État animation par calque : `animation: {enabled: true, domain: null}` (`trip-layer.ts:527-530`).

### 3.2 Détection du format et conversion du temps

- `isTripGeoJsonField` (`trip-utils.ts:35-69`) : au plus 10 000 lignes échantillonnées (`maxCount`, `:40`). Trois conditions :
  1. au moins une LineString (`:53`) ;
  2. **toutes** les coordonnées échantillonnées ont au moins 4 éléments (`coordHasLength4`, `:15-27`) ;
  3. les 4ᵉ éléments de la **première** feature passent `containValidTime` (`:62-68`).
- `containValidTime` (`src/common-utils/src/data.ts:68-81`) délègue à `type-analyzer@0.4.0` (`Analyzer.computeColMeta`) en ignorant tous les types sauf TIME/DATETIME/DATE. Le résultat porte un `format`.
- `parseTripGeoJsonTimestamp` (`trip-utils.ts:76-112`) : le format est analysé **sur un seul trajet échantillon**, le premier qui a plus de 3 coordonnées (`:86`), puis appliqué à tous les trajets. La conversion passe par `timeToUnixMilli(coord[3], format)`.
- `timeToUnixMilli` (`src/utils/src/data-utils.ts:130-144`) :
  - chaîne → `moment.utc(value, format)` ;
  - nombre → `format === 'x' ? value * 1000 : value` ;
  - Date → `valueOf()`.
- **Attention à la convention inversée** : chez type-analyzer, `'x'` = secondes Unix et `'X'` = millisecondes. C'est l'inverse de moment.js. Preuve : dans `test/fixtures/state-saved-v1-2.js:4991-4992`, `epoch → 'x'` et `epoch_ms → 'X'` ; dans `test/fixtures/test-csv-data.js:8,497`, la valeur `1472688000000` (13 chiffres) est typée `'X'`.
- La doc utilisateur limite les entrées à « unix en secondes `1564184363` ou en ms `1564184363000` » (`docs/user-guides/c-types-of-layers/k-trip.md`).
- Domaine : `getAnimationDomainFromTimestamps` (`trip-utils.ts:134-152`) = [min des premiers ts valides, max des derniers ts valides] sur tous les trajets. Le code **suppose les ts triés** (`findMinFromSorted`/`findMaxFromSorted`, `:114-132`).

### 3.3 Horloge globale `animationConfig`

- Type `AnimationConfig` (`src/types/reducers.d.ts:360-375`) : `domain [ms,ms] | null`, `currentTime ms`, `speed`, `duration`, `isAnimating`, `timeSteps`, `defaultTimeFormat`, `timeFormat`, `timezone`, `hideControl`.
- Défauts : `DEFAULT_ANIMATION_CONFIG` (`src/reducers/src/vis-state-updaters.ts:361-372`), `speed: 1`, tout le reste à null ou false.
- Domaine global : `updateAnimationDomain` (`vis-state-updaters.ts:5173-5241`) fusionne les domaines de tous les calques animables visibles (`mergeTimeDomains`, `src/utils/src/filter-utils.ts:1588-1597`, union min/max). Il ramène `currentTime` dans le domaine si besoin (`:5231-5237`).
- `setLayerAnimationTimeUpdater` (`vis-state-updaters.ts:1357-1376`) ne fait qu'écrire `currentTime`. Il **exclut explicitement les calques `trip`** du recalcul `layerAnimationChangeUpdater` (`:1371`, `l.type !== 'trip'`). Pour une traînée, avancer le temps ne recalcule donc aucune donnée : seul un uniform GPU change (cf. §5.2).

### 3.4 Filtre temporel `TimeRangeFilter`

- Type (`reducers.d.ts:170-212`, `:239-257`) : `domain`, `value [t0,t1]`, `view: 'side'|'enlarged'|'minified'`, `fixedDomain`, `isAnimating`, `speed`, `animationWindow`, `syncedWithLayerTimeline`, `syncTimelineMode: 0|1`, `plotType` (histogramme ou courbe, `interval`), `yAxis`, `invertTrendColor`, `timezone`.
- Il gère aussi l'**intervalle** `endName/endFieldIdx/endMappedValue/gpuEndChannel` : une feature reste visible tant que `[start, end]` chevauche la fenêtre (`:252-256`, doc `docs/user-guides/h-playback.md` § Start and end time).
- Défaut pour un champ timestamp (`filter-utils.ts:471-479`) : `type timeRange`, `view enlarged`, `fixedDomain true`, `value = domain`, `gpu true`.
- **`fixedDomain`** : le filtre range ses filtres dans `fixedDomain` ou `dynamicDomain` (`filter-utils.ts:716-744`). Un filtre à domaine fixe ne voit **pas** son domaine recalculé quand d'autres filtres changent les lignes visibles. La frise ne « saute » donc pas quand on filtre autre chose.
- GPU : au plus 4 filtres GPU (`MAX_GPU_FILTERS = 4`, `src/constants/src/default-settings.ts:1362`). Les valeurs envoyées au GPU sont **relatives à `domain[0]`** (`src/table/src/gpu-filter-utils.ts:274-276`, `value - filter.domain[0]`). Pour le Trip layer en mode table, la valeur filtrée est un **tableau par sommet** (`:278-288`, « relevant for TripLayer »), donc le filtrage se fait par point.

### 3.5 Sérialisation versionnée

- `src/schemas/src/versions.ts:6-11` : `VERSIONS = {v0, v1}`, `CURRENT_VERSION = v1`. Chaque sous-schéma porte sa version et un `save/load` dédié.
- Pour l'animation, `animationConfig` ne sauvegarde **que** `currentTime, speed, timeFormat, timezone` (`vis-state-schema.ts:1081-1090`). Le **domaine n'est jamais persisté** : il est recalculé depuis les données au chargement.
- Filtres v1 (`vis-state-schema.ts:999-1030`) : `plotType, animationWindow, yAxis, view, speed, syncedWithLayerTimeline, syncTimelineMode, enabled, invertTrendColor, timezone, endName` s'ajoutent à v0 (`dataId, id, name, type, value, enlarged`, `:784-792`).
- Migration en douceur : `enlarged` (booléen v0) est converti en `view` au chargement (`vis-state-schema.ts:659-667`).
- Calque : `config.visConfig` (qui contient `trailLength`, `fadeTrail`), `columns`, `columnMode` (`vis-state-schema.ts:567-598`). `animation.domain` n'est pas sérialisé.
- Rien sur la provenance ni l'incertitude temporelle : un timestamp est un instant exact, l'intervalle n'existe que via `endName`.

## 4. Patterns UX observés

**Captures publiques (doc officielle)** :
- traînée animée : https://d1a3f4spazzrp4.cloudfront.net/kepler.gl/documentation/k-trip.gif ;
- couleur par attribut : https://d1a3f4spazzrp4.cloudfront.net/kepler.gl/documentation/k-trip-attribute-colors.png ;
- frise et histogramme : https://d1a3f4spazzrp4.cloudfront.net/kepler.gl/documentation/h-playback-1.png ;
- réglage de vitesse : https://d1a3f4spazzrp4.cloudfront.net/kepler.gl/documentation/h-playback-2.gif ;
- axe Y personnalisé : https://d1a3f4spazzrp4.cloudfront.net/kepler.gl/documentation/h-playback-3.png.

**Images dans le dépôt** : seulement `screenshots/screenshot.png` (vue générale). Aucune capture trip ou playback n'est versionnée.

1. **Création automatique du calque Trip** : quand on dépose un GeoJSON 4D, `findDefaultLayerProps` (`trip-layer.ts:445-521`) crée un calque Trip et **supprime le calque GeoJSON** créé en parallèle sur la même colonne (`:470-476`). Pour un CSV, il détecte `id/lat/lng/timestamp` (`:480-517`). L'utilisateur voit directement une animation, sans configuration.
2. **Deux barres en bas de carte**, `src/components/src/bottom-widget.tsx:89-230` :
   - la *timeline des calques* (`LayerAnimationController`) apparaît dès qu'un calque animable est visible (`:115-130`). Elle se cache si on masque le calque (doc k-trip.md « Upon hiding the trip layer, its animation control will also hide ») ;
   - le *TimeWidget* d'un filtre temporel en vue `enlarged` (`:131`, `:197-230`).
   - Si le filtre est synchronisé, la timeline des calques est masquée et les deux horloges n'en font plus qu'une (`:95-99`).
3. **Lecture et vitesse** : bouton play/pause/reset et vitesse affichée `1.0x` (`speed-control.tsx:41`). Curseur de 0 à 10, pas de 0.001 (`SPEED_CONTROL_RANGE`, `SPEED_CONTROL_STEP`, `default-settings.ts:1317-1318`). La vitesse **n'est pas du temps réel** (cf. §5.3 et §7).
4. **Quatre modes de fenêtre** (`default-settings.ts:1288-1315`, doc h-playback.md § Animation window modes) :
   - `free` : fenêtre de taille fixe qui glisse ;
   - `incremental` : début figé, la fin avance, les éléments s'accumulent ;
   - `point` : instant (horloge des calques) ;
   - `interval` : saut de bac d'histogramme en bac.
   On choisit le mode avec `animation-window-control.tsx`.
5. **Zoom de frise à la molette** (`src/components/src/filters/time-widget.tsx:357-404`) :
   - **molette** = redimensionne la *fenêtre* autour du curseur souris (pas 8 %, largeur minimale = domaine/100, `:220-247`) ;
   - **Ctrl/⌘ + molette ou pincement** = zoome la *frise elle-même* (pas 2 %, largeur minimale = domaine/20, `:264-333`). L'état zoomé est local au composant (`useState`, `:171`) et un bandeau « Showing … Reset » apparaît (`timelineZoomed`, `:192-198`) ;
   - événements throttlés à 40 ms (`:335-343`) ;
   - la molette est ignorée quand le pointeur est sur un champ de saisie (`:367-374`).
6. **Clavier** : Ctrl/⌘ + ←/→ décale la fenêtre **de sa propre largeur** (pagination), ou d'un bac en mode interval (`time-widget.tsx:406-459`). Raccourci ignoré si le focus est dans un input. La doc annonce aussi Maj pour un grand pas (h-playback.md), mais le handler ne lit pas `shiftKey` : écart doc/code.
7. **Histogramme sous la fenêtre** : bacs alignés sur des intervalles calendaires d3 (`utcHour`, `utcDay`…, `src/constants/src/time.ts:114-123`). Les bacs sont calculés **sans appliquer le filtre lui-même** (`runGpuFilterForPlot`, `src/utils/src/plot.ts:280-307`), ce qui montre la densité hors fenêtre. Un axe Y personnalisé transforme l'histogramme en série temporelle (doc h-playback.md §4).
8. **Tooltip d'un trajet en mode table** : il affiche la ligne de donnée **à l'instant courant**, pas la première (`getHoverData` → `_findDatumForFeatureByTime(...).datum`, `trip-layer.ts:546-552`).
9. **Modèle 3D en tête de trajet** (option `scenegraph`) : un glTF orienté par le vecteur vitesse (`_getYaw`, `trip-layer.ts:990-1016`), avec interpolation angulaire `interpolateAngle` (`:1159-1163`).

## 5. Algorithmes / mécanismes réutilisables

### 5.1 Normalisation des timestamps (kepler)

```
format = type_analyzer(ts du 1er trajet ayant >3 coords)   # 'x'=s, 'X'=ms, ou motif moment
pour chaque trajet, chaque coord :
    ts_ms = string ? moment.utc(ts, format) : (format=='x' ? ts*1000 : ts)
domain = [min_i ts_i[0 valide], max_i ts_i[dernier valide]]   # suppose ts triés
```
Coût O(N) sur le nombre total de points. Cas limites non gérés : format différent selon les trajets, ts non triés (mode GeoJSON), ts dupliqués, décimales (cf. §7).

### 5.2 Rendu de la traînée : deck.gl `TripsLayer` (shader)

kepler passe à deck (`trip-layer.ts:782-815`) :
```
domain0      = animationConfig.domain[0]
getTimestamps= d => ts_ms[d] - domain0          # relatif pour la précision float32
currentTime  = animationConfig.currentTime - domain0
trailLength  = visConfig.trailLength * 1000     # secondes → ms
fadeTrail    = visConfig.fadeTrail
updateTriggers.getTimestamps = {columns, domain0}   # ré-upload seulement si domain0 change
```
`TripsLayer` hérite de `PathLayer`. L'attribut `timestamps` (1 float par sommet) est lié deux fois avec `vertexOffset` 0 et 1, pour obtenir `instanceTimestamps` et `instanceNextTimestamps` par segment. Les shaders (deck.gl `modules/geo-layers/src/trips-layer/trips-layer.ts`) :
```glsl
// vertex : temps interpolé le long du segment
vTime = instanceTimestamps + (instanceNextTimestamps - instanceTimestamps)
        * vPathPosition.y / vPathLength;
// fragment
if (vTime > currentTime || (fadeTrail && vTime < currentTime - trailLength)) discard;
if (fadeTrail) color.a *= 1.0 - (currentTime - vTime) / trailLength;
```
- Le fondu est **linéaire par âge** du fragment : alpha 1 à la tête, 0 à `currentTime - trailLength`.
- Avec `fadeTrail = false`, rien n'est coupé en arrière : on voit **tout le trajet jusqu'au curseur**. C'est exactement « trajets jusqu'au curseur » du PRD.
- Défauts deck : `trailLength 120`, `currentTime 0`, `fadeTrail true`.
- Coût par frame : **zéro upload**. Seul l'uniform `currentTime` change : O(1) côté CPU, O(sommets) côté GPU.
- La tête (position exacte) n'est pas dessinée par TripsLayer : la traînée s'arrête au fragment où `vTime == currentTime`. La position d'un marqueur ou d'un label vient du CPU (§5.4).

### 5.3 Horloge d'animation (`animation-controller.ts`)

```
à chaque requestAnimationFrame :
  delta = (domain[1]-domain[0]) / BASE_SPEED(600) * speed
  point : t = t+delta > domain[1] ? domain[0] : t+delta          (boucle)
  free  : [a,b] += delta ; si a+delta > domain[1] → a = domain[0]-largeur
  incremental : b = min(b+delta, fin du dernier bac) ; boucle à [a, a+1]
  interval : saut au bac suivant toutes les (600*1000/60)/n_bacs/speed ms
```
Source : `src/components/src/common/animation-control/animation-controller.ts:99-115`, `:190-256`, constantes `BASE_SPEED=600`, `FPS=60` (`default-settings.ts:1285-1286`). Une lecture complète dure donc toujours 600 frames, soit ≈ 10 s à 60 Hz, **quelle que soit la durée réelle du domaine**. Elle ralentit si le navigateur tombe à 30 fps. Chaque frame dispatche une action Redux (`setLayerAnimationTime` / `setFilterAnimationTime`).
Avant écriture, l'horloge des calques est **accrochée aux pas** si `timeSteps` existe (`snapToMarks`, `plot.ts:466-480`), sinon bornée au domaine (`layer-animation-controller.tsx:36-48`).

### 5.4 Position interpolée au curseur (`_findDatumForFeatureByTime`, `trip-layer.ts:1051-1095`)

```
idx = bisectRight(ts, t)                       # O(log n)
si idx >= n : idx = n-1
si ts[0] <= t <= ts[n-1] :
    adv = idx>0 ? (t - ts[idx-1]) / (ts[idx]-ts[idx-1]) : 0
    pos = lerp(coord[idx-1].xyz, coord[idx].xyz, adv)
    datum = coord[idx].datum ; prevDatum = coord[idx-1].datum
sinon : hors trajet → pos = null (marqueur masqué, z = -MAX en GPU)
```
C'est l'interpolation « côté client, sans requête par frame » que veut le PRD : O(A log n) par frame pour A acteurs. Cas limite : `ts[idx] == ts[idx-1]` donne une division par zéro, d'où `adv` = NaN/∞ et une position NaN (aucune garde). Lacet : `atan2` en coordonnées Web Mercator entre deux points consécutifs (`getYawFromSpeedVector`, `:1137-1149`). `getDeckRenderCallbackData` (`:1097-1130`) exporte pour chaque trajet `{position, orientation, keyframes}` à `currentTime`, prêt à brancher un rendu externe.

### 5.5 Synchronisation filtre ↔ calques (`syncTimeFilterWithLayerTimelineUpdater`, `vis-state-updaters.ts:6196-6319`)

```
activation :
  domaine_filtre = union(domaine_filtre, animationConfig.domain)
  si calque animé par intervalle : fenêtre → 'interval', alignée sur les bacs
  valeur_filtre ajustée au nouveau domaine ; syncedWithLayerTimeline = true
  currentTime = valeur_filtre[syncTimelineMode]    # 0 = bord début, 1 = bord fin
désactivation : mode = fin, valeur réinitialisée, currentTime = domain[0]
à chaque changement de filtre synchronisé : adjustAnimationConfigWithFilter
  → currentTime = value[mode] (accroché aux timeSteps)        (:6418-6430)
```
Le bord « début » n'est permis qu'en fenêtre `free` (`validateSyncAnimationMode`, `:6444-6448`). Dans `updateAnimationDomain`, le domaine d'animation fusionne celui du filtre synchronisé (`:5226-5233`).

### 5.6 Bacs temporels (`getBinThresholds`, `plot.ts:331-351`)

`floor(t0)` à `ceil(t1)` par intervalle d3 UTC. On ajoute un pas si le dernier seuil vaut exactement `t1`. `intervalBinFromMarks` (`:447-464`) renvoie `[bin_start, next_bin_start - 1]` (comparaison inclusive). C'est réutilisable tel quel pour les graduations et la densité de la frise PNEX.

### 5.7 Transposition à MapLibre seul (PNEX : maplibre-gl 6.10, bundle esbuild IIFE `crates/pnex-frontend/assets/map.js` ≈ 2,28 Mo non minifié)

Contraintes vérifiées dans la spec MapLibre (https://maplibre.org/maplibre-style-spec/layers/) :
- `line-gradient` exige une source GeoJSON avec `lineMetrics: true`.
- `line-gradient` ne supporte **pas** le data-driven (« Not supported yet » en GL JS) : un seul dégradé par calque.
- `line-progress` mesure une **distance** le long de la ligne (0→1), **pas le temps**.
- Aucune propriété `line-trim-offset` dans MapLibre (elle existe chez Mapbox GL).
- L'expression `["global-state", name]` est disponible en GL JS depuis 5.6.0 (https://maplibre.org/maplibre-style-spec/expressions/).

| Option | Principe | Coût par frame | Fidélité | Poids JS |
|---|---|---|---|---|
| A. Découpe client + `setData` | fenêtre de traces chargée une fois en `Float64Array` (ts) + coords. À chaque frame, par acteur : bisect, sous-polyligne `[t-trail, t]` + tête interpolée. Âge découpé en K tranches (K=6 à 8) → features `{a: alpha}`, `line-opacity: ["get","a"]` (data-driven OK) ; la tête en `circle` | `setData` d'une petite source (seulement les points visibles) → geojson-vt dans le worker ; O(points visibles) | fondu par paliers (K tranches), exact en temps | 0 |
| B. `line-gradient` | une couche par acteur, `lineMetrics`. Le dégradé est recalculé par `setPaintProperty` avec des arrêts en `line-progress` issus d'une table temps→distance cumulée | ré-évaluation du dégradé, pas de re-tuilage | dégradé continu ; cut en distance (correct si on convertit t→progress) ; résolution de la texture de dégradé à vérifier sur les longues traces | 0 |
| C. `global-state` + segments | 1 feature par segment `{t0,t1}`, `filter`/`line-opacity` lisent `["global-state","t"]` | aucun upload ; coût de ré-évaluation des filtres/paint côté worker **à mesurer** | bon | 0 (MapLibre ≥ 5.6) |
| D. `@deck.gl/mapbox` `MapboxOverlay({interleaved:true})` + `TripsLayer` | shader §5.2, contexte WebGL partagé avec MapLibre | uniform seulement | identique à kepler, fondu continu, 10⁵–10⁶ sommets | voir ci-dessous |

Poids mesurés sur les UMD minifiés unpkg 9.1.14 (gzip -9) :
- `@deck.gl/core` 723 Ko / **193 Ko gz** (luma.gl inclus) ;
- `@deck.gl/layers` 155 Ko / **41 Ko gz** ;
- `@deck.gl/mapbox` 10 Ko / 4 Ko gz ;
- `@deck.gl/geo-layers` complet 580 Ko / 169 Ko gz. Importer seulement `TripsLayer` via esbuild élague l'essentiel : TripsLayer est un petit fichier au-dessus de PathLayer.
- Coût réel ajouté ≈ **+250 Ko gz** (core + layers + mapbox + TripsLayer), à comparer à MapLibre 5.6 ≈ 248 Ko gz. Soit environ deux fois le poids carte.
- Licence MIT, compatible.

**Avis argumenté** :
- **Option A par défaut.** Un dossier d'enquête compte typiquement 1 à 20 acteurs et quelques milliers de points par fenêtre, sur Raspberry Pi 5 et téléphone. A n'ajoute aucune dépendance et reste dans le pipeline de style de MapLibre : thème clair/sombre, z-order avec POI et geofences, picking cohérent avec le reste de la carte. Elle suit exactement le PRD : chargement par fenêtre, puis interpolation client.
- La découpe et l'interpolation se codent **en Rust côté wasm** : un `struct Track { ts: Vec<i64>, xy: Vec<[f64;2]> }`, bisect, lerp. On ne passe à JS qu'un GeoJSON minimal par frame, throttlé à 30 fps. Le « trajet complet jusqu'au curseur » (l'équivalent de `fadeTrail=false`) est une 2ᵉ couche pâle alimentée par la même découpe `[domain0, t]`.
- **B** en complément si l'on veut un fondu continu pour un petit nombre d'acteurs.
- **C** est à prototyper : c'est la plus élégante, mais son coût de ré-évaluation est inconnu.
- **D** seulement si un cas « flotte » apparaît (centaines de véhicules, 10⁵+ sommets visibles). Le prix : un 2ᵉ moteur de rendu, deux systèmes de picking et de thème, et la compatibilité deck 9.x × MapLibre 6 à vérifier avant d'y aller.

## 6. 3 patterns à reprendre dans PNEX

### P1. Temps canonique en ms entières, relatif à `domain0` au moment du rendu
- **Quoi** : kepler convertit tout en ms Unix (`timeToUnixMilli`) puis soustrait `domain0` avant tout passage en float32 (deck `getTimestamps`, `trip-layer.ts:800,813` ; filtres GPU, `gpu-filter-utils.ts:274-276`). `trailLength` est saisi en secondes et converti ×1000 en un seul endroit (`:810`).
- **Pourquoi** : en float32, un epoch en ms (~1,7·10¹²) perd toute résolution sous la minute. Relatif au début du dossier, on garde une précision à la ms sur ~4,6 h et à environ 4 s sur un an.
- **Comment dans PNEX** : côté Rust, `ts: i64` ms UTC dans le DTO de trace (lu depuis O2, jamais copié). Dans le front wasm, on garde un `Float64Array` pour la logique et on n'envoie à MapLibre que des valeurs relatives (`t - case_t0`) dans les propriétés de features (B/C) ou des géométries déjà découpées (A). Le format kepler `[lon, lat, alt, ts]` reste un **format d'échange** (import/export), pas un stockage.

### P2. Deux horloges, point et fenêtre, unifiées par un mode de synchronisation
- **Quoi** :
  - l'horloge des calques est un **instant** (`currentTime`, fenêtre `point`) ;
  - le filtre est une **période** (`value [t0,t1]`, fenêtres `free/incremental/interval`) ;
  - quand on les synchronise, l'instant devient un bord de la période (`syncTimelineMode` 0 = début, 1 = fin ; §5.5).
  - Ergonomie associée : molette = largeur de période autour du pointeur, Ctrl + molette = zoom de la règle, Ctrl+flèches = pagination d'une largeur de période, zoom de règle local (non persisté) avec bouton Reset.
- **Pourquoi** : c'est exactement la frise du PRD (clic = instant, glisser = période). Le mode « fin » donne naturellement « trajets jusqu'au curseur, période = contexte ». Le mode `incremental` donne l'accumulation.
- **Comment** : un seul signal Dioxus `TimeCursor { t: i64, window: Option<(i64,i64)>, anchor: Edge::End }`. La carte lit `t`, la liste et le graphe lisent `window`. Le zoom de la règle reste dans un signal local du composant frise, pas dans le dossier. Raccourcis : espace = play/pause, ←/→ = pas, Ctrl+←/→ = pagination, Échap = sortir de la période. On reprend les constantes kepler (pas 8 % / 2 %, largeur minimale domaine/100 et domaine/20, throttle 40 ms) comme valeurs de départ.

### P3. Interpolation au curseur par bisect + lerp, et rien de recalculé par frame
- **Quoi** : `_findDatumForFeatureByTime` (§5.4) donne la position, l'attribut courant (`datum`), l'attribut précédent et l'avancement. kepler sort le Trip layer du recalcul de données par frame (`vis-state-updaters.ts:1371`) : seul `currentTime` bouge.
- **Pourquoi** : c'est la garantie de « pas de requête par frame » et d'un coût O(A log n). Le même calcul sert à la tête sur la carte, à l'inspecteur « le dossier à HH:MM » (valeur du capteur à l'instant) et aux tooltips.
- **Comment** : un module Rust partagé (crate core, compilé en wasm et utilisé côté serveur par les détecteurs) : `fn sample(track, t) -> Option<Sample{pos, idx, adv}>`. Il doit **gérer les trous de signal** : au-delà d'un seuil `max_gap`, renvoyer `None` plutôt que lerper à travers un trou (kepler lerpe toujours entre deux points). Il doit aussi gérer `ts[idx]==ts[idx-1]` (renvoyer le point). On l'accompagne d'un test unitaire bisect/lerp/trou/doublon. Le gel (snapshot SHA-256) sérialise `t`, `window`, `speed` **et** le domaine : contrairement à kepler qui recalcule le domaine au chargement, un gel doit être reproductible même si les données bougent.

## 7. 3 pièges à éviter

### Piège 1. Une vitesse qui n'est pas du temps réel et qui dépend du framerate
- **Problème** : `delta = domaine/600 × speed` **par requestAnimationFrame** (`animation-controller.ts:202-207`). « 1x » ne veut pas dire « temps réel » : c'est « tout le domaine en 600 frames ». Un domaine d'une semaine et un domaine de dix minutes durent le même temps. À 30 fps, la lecture dure deux fois plus longtemps. En plus, une action Redux est dispatchée à chaque frame. En mode `interval`, la temporisation est faite par une boucle rAF qui compare `Date.getTime()` (`:99-115`).
- **Preuve** : code cité ; issue #740 (« Adding a input box for animation speed specification ») ; doc h-playback.md (« 1x, 2x, 4x » sans référence au temps réel).
- **Parade PNEX** : le replay ×1/×3/×8 du PRD est du **temps réel multiplié** : `t += (now - last_now) × facteur` avec `performance.now()`. Il ne dépend plus du framerate et rattrape tout seul les frames perdues. Ajouter un mode « ajusté » optionnel (domaine en N secondes) pour les longues périodes. L'horloge vit dans un signal local, pas dans un store global réécrit à chaque frame. Le rendu est throttlé à 30 fps sur le Pi.

### Piège 2. Une détection de timestamps fragile et une entrée non normalisée
- **Problème** :
  - le format est déduit du **premier trajet seulement**, par les regex de type-analyzer (`trip-utils.ts:86-99`) ;
  - le mode GeoJSON ne trie pas les points et le domaine suppose des ts triés (`trip-utils.ts:114-132`) ;
  - le mode table trie sur la valeur **brute** : une chaîne « 07/30/2024 » se trie lexicographiquement (`geojson-utils.ts:542-544`) ;
  - aucune garde contre deux ts égaux (division par zéro dans `_findDatumForFeatureByTime`, `trip-layer.ts:1069-1070`).
- **Preuves (issues GitHub)** :
  - #2526 : ts décimaux `1712041205.47` non reconnus, pas d'animation ; réponse mainteneur : « time coordinates should be integers » ;
  - #2287 : petits entiers `25618` non reconnus comme temps, le calque bascule en GeoJSON simple ;
  - #1117 : ts non monotones (`…521285, …491271`) ;
  - #2813 (ouverte) : format « 07/30/2024 11:36:02 » en mode table, erreur deck « Cannot read properties of undefined (reading 'values') » ;
  - #1124 : plantage avec moins de 3 coordonnées ;
  - côté deck : #6844, deux coordonnées identiques → `vPathLength = 0` → `vTime` en division par zéro, « fireflies ».
- **Parade PNEX** : normaliser **à la frontière, en Rust** (import CSV/GPX et lecture O2) :
  - ts → `i64` ms UTC, avec l'unité **déclarée** par la source ou déduite par magnitude sur **toute** la colonne (≤ 1e11 → s, ≤ 1e14 → ms, ≤ 1e17 → µs), refus explicite sinon ;
  - tri stable par ts ;
  - fusion des doublons (même ts → dernier point) et retrait des points immobiles consécutifs ;
  - trajet d'un seul point accepté (affiché en position, sans traînée) ;
  - erreur de champ en code machine (règle i18n PNEX) au lieu d'un silence.
  Le front ne reçoit que des traces propres.

### Piège 3. Des perfs et une précision qui cassent à l'échelle
- **Problème** :
  - pas de chemin binaire pour les trajets : `calculateDataAttribute` ne garde que des objets `LineString` (« TODO d can be BinaryFeatureCollection », `trip-layer.ts:557-566`). Les `MultiLineString` sont silencieusement ignorées et chaque trajet est un objet JS. En mode table, `values: items.map(rowAsArray)` matérialise chaque ligne (`geojson-utils.ts:548-550`) ;
  - `trailLength` est plafonné par le slider à [1, 1000] **secondes** (`layers.ts:122-131`), avec un libellé erroné `layerVisConfigs.strokeWidth` (`:125`). Trop court à l'échelle d'un continent ;
  - la précision float32 des ts dans le shader reste limite même relative.
- **Preuves** :
  - kepler #2939 (ouverte) : demande geoarrow pour Trip ;
  - kepler #2038 : export HTML > 400 Mo saccadé à 15 000 trajets ;
  - kepler #2089 : « Trail length is too short » ;
  - deck #8927 (ouverte) : « TripsLayer's vTime has rendering problems when the segments are short with precise timestamps » ;
  - deck #10387 : artefact sur le dernier segment (MapboxOverlay) ;
  - kepler #3094 : un survol de traînée longue faisait planter les tooltips jusqu'au rechargement.
- **Parade PNEX** :
  - ne jamais transférer « tout » : la fenêtre de la frise borne la requête O2, avec simplification Douglas-Peucker côté serveur selon le zoom ;
  - la trace découpée (option A) ne contient que les points visibles ;
  - longueur de traînée exprimée **en durée libre** (champ, pas slider borné), avec une valeur par défaut proportionnelle à la largeur de la fenêtre de frise (par ex. 10 %) ;
  - temps relatif à `case_t0` en f64 côté wasm ;
  - picking via `queryRenderedFeatures` sur des features porteuses de l'`object_id` : jamais d'index positionnel fragile.

## 8. Réponses aux questions ouvertes du PRD

- **(a) Traces importées CSV/GPX vs flux** : kepler accepte les deux formes **statiques** : GeoJSON 4D et table de points `id/lat/lng/ts` regroupée et triée au chargement (`groupColumnsAsGeoJson`). Il n'a pas de flux : le domaine est recalculé à chaque changement de données (`updateLayerMeta` → `updateAnimationDomain`) et n'est pas persisté. Ce qu'on en retient pour PNEX :
  - le **stockage canonique est la table de points** : c'est déjà la forme des séries O2 et des imports CSV/GPX ;
  - la LineString 4D façon kepler est une **projection à la lecture** par fenêtre : c'est le format d'échange et d'export ;
  - un import est une source avec provenance (fichier, SHA-256). Un flux device est lu à la volée sur la même interface `Track`, sans copie. Le domaine du dossier se recalcule depuis les attachements, sauf dans un gel où il est figé.
- **(b) Graphe natif vs Cytoscape.js en wasm** : kepler n'apporte rien (pas de graphe). Seul enseignement transposable : ce qui bouge à chaque frame est un **paramètre global unique** (uniform `currentTime`), pas une mise à jour d'éléments. Pour l'estompage temporel du graphe, mieux vaut un seul `t` lu par le style que des mises à jour nœud par nœud.
- **(c) Nommage** : kepler parle de « Trip », « Playback », « Time Playback », « Filter ». C'est un vocabulaire d'outil de visualisation, sans notion d'enquête. Rien à en tirer pour « Dossier d'enquête / Enquête / Projet ». Seule remarque utile : kepler sépare le **réglage de calque** (« Trail length ») de la **lecture** (« Playback »). Dans PNEX, « Replay » ou « Lecture » pour la frise et « Traînée » pour la carte resteraient distincts du nom du conteneur.

## 9. Sources

**Fichiers lus (commit c9f6ecb)** :
- `LICENSE`, `src/sqlrooms/LICENSE`, `package.json`, `src/layers/package.json`, `src/components/package.json`
- `src/layers/src/trip-layer/trip-layer.ts` (intégral, 1163 l.), `src/layers/src/trip-layer/trip-utils.ts` (intégral)
- `src/layers/src/geojson-layer/geojson-utils.ts:255-330, 494-640`
- `src/common-utils/src/data.ts:55-81`, `src/utils/src/data-utils.ts:120-144`
- `src/utils/src/filter-utils.ts:95-125, 455-480, 710-790, 1588-1625`, `src/utils/src/plot.ts:280-490`
- `src/table/src/gpu-filter-utils.ts:230-290` (+ index des fonctions), `src/table/src/tileset/vector-tile-utils.ts:480-495`
- `src/reducers/src/vis-state-updaters.ts:355-375, 1350-1430, 5165-5245, 6190-6460`
- `src/components/src/common/animation-control/animation-controller.ts` (intégral), `speed-control.tsx`, `animation-speed-slider.tsx`
- `src/components/src/layer-animation-controller.tsx`, `src/components/src/filter-animation-controller.tsx`, `src/components/src/bottom-widget.tsx:85-230`
- `src/components/src/filters/time-widget.tsx:130-470`, `src/components/src/filters/time-range-filter.tsx`, `src/components/src/common/time-range-slider.tsx`
- `src/constants/src/default-settings.ts:440-452, 1280-1330, 1362, 1905-1918`, `src/constants/src/layers.ts:118-145`, `src/constants/src/time.ts:100-135`
- `src/types/reducers.d.ts:160-400`
- `src/schemas/src/versions.ts`, `src/schemas/src/vis-state-schema.ts:567-605, 650-675, 784-792, 995-1095`
- `test/fixtures/test-csv-data.js:7-10, 490-500`, `test/fixtures/state-saved-v1-2.js:4991-5022`
- `docs/user-guides/c-types-of-layers/k-trip.md`, `docs/user-guides/h-playback.md`, `docs/user-guides/e-filters.md`
- PNEX (contexte) : `crates/pnex-frontend/package.json`, taille de `crates/pnex-frontend/assets/map.js`

**URLs** :
- deck.gl TripsLayer (shaders, defaultProps) : https://raw.githubusercontent.com/visgl/deck.gl/master/modules/geo-layers/src/trips-layer/trips-layer.ts
- MapLibre style spec, couches : https://maplibre.org/maplibre-style-spec/layers/ ; expressions : https://maplibre.org/maplibre-style-spec/expressions/
- Poids : https://unpkg.com/@deck.gl/core@9.1.14/dist.min.js, https://unpkg.com/@deck.gl/layers@9.1.14/dist.min.js, https://unpkg.com/@deck.gl/geo-layers@9.1.14/dist.min.js, https://unpkg.com/@deck.gl/mapbox@9.1.14/dist.min.js, https://unpkg.com/deck.gl@9.1.14/dist.min.js, https://unpkg.com/maplibre-gl@5.6.0/dist/maplibre-gl.js
- Issues kepler.gl : https://github.com/keplergl/kepler.gl/issues/2813, /2526, /2287, /1117, /1124, /1685, /2524, /2038, /2089, /2939, /3094, /740
- Issues deck.gl : https://github.com/visgl/deck.gl/issues/8927, /6844, /10387
- Captures doc : https://d1a3f4spazzrp4.cloudfront.net/kepler.gl/documentation/k-trip.gif, …/k-trip-attribute-colors.png, …/h-playback-1.png, …/h-playback-2.gif, …/h-playback-3.png
